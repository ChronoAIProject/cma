use super::*;
use crate::store::sandbox_mongo::{is_transient, start_transaction};
use std::collections::{BTreeMap, BTreeSet};

async fn contended(release: bool, expires: bool) {
    let (client, name) = database("binding_contention").await;
    let (store, agent, parent, _, _, _) = composed(&client, &name).await;
    let (binding, conversation, _) = binding_for(&store, &agent).await;
    let mut first = reservation(&store, &agent, &parent, &binding).await;
    let AgentCommand::Message { input, .. } = &mut first.envelope.command else {
        unreachable!()
    };
    input.content.push(AgentContent::ExternalEvent {
        context: ExternalEventContext {
            provider_conversation_id: None,
            source_id: "independent-channel".into(),
            conversation_id: conversation.conversation_id,
            event_id: "contended-event".into(),
            reference: "sealed-reference".into(),
            payload: serde_json::json!({"body":"x".repeat(300_000)}),
        },
    });
    first.envelope.request_fingerprint = first.envelope.command.fingerprint().unwrap();
    let key = first.envelope.external_event_key().unwrap();
    let mut second = first.clone();
    second.key_sha256 = command_key_digest("contending-event-key").unwrap();
    second.envelope.command_id = AgentCommandId::generate();
    second.response_id = Some(AgentResponseId::generate());

    // Count distinct transactions, since driver commands can emit multiple
    // failures for one attempt. Release is caused by actual write conflicts.
    let conflicts = Arc::new(Mutex::new((
        BTreeMap::<i32, String>::new(),
        BTreeSet::<String>::new(),
    )));
    let observed = conflicts.clone();
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut state = observed.lock().unwrap();
        match event {
            CommandEvent::Started(event)
                if matches!(event.command.get_bool("autocommit"), Ok(false)) =>
            {
                state.0.insert(
                    event.request_id,
                    format!(
                        "{:?}:{:?}",
                        event.command.get("lsid"),
                        event.command.get("txnNumber")
                    ),
                );
            }
            CommandEvent::Failed(event) => {
                if let Some(identity) = state.0.remove(&event.request_id) {
                    if is_transient(&event.failure) {
                        state.1.insert(identity);
                    }
                }
            }
            CommandEvent::Succeeded(event) => {
                state.0.remove(&event.request_id);
            }
            _ => {}
        }
    }));
    let replica = Client::with_options(options).unwrap();
    let contender = MongoSandboxStore::open(&replica, &name).await.unwrap();
    let mut holder = client.start_session().await.unwrap();
    start_transaction(&mut holder).await.unwrap();
    let accepted =
        match crate::store::agent_commands::reserve::apply(&store, &first, &mut holder).await {
            Ok(result) => record(result),
            Err(_) => panic!("fixture must reserve the winning command"),
        };
    assert!(store
        .get_external_event_command(&key)
        .await
        .unwrap()
        .is_none());
    if expires {
        second.authority_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(200));
    }
    let retry = second.clone();
    let started = tokio::time::Instant::now();
    let task = tokio::spawn(async move { contender.reserve_command(second).await });
    if release {
        let observed = tokio::time::timeout(Duration::from_secs(4), async {
            while conflicts.lock().unwrap().1.len() < 6 && !task.is_finished() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        holder.commit_transaction().await.unwrap();
        observed.unwrap();
    }
    let result = tokio::time::timeout(Duration::from_secs(9), task).await;
    if !release {
        holder.commit_transaction().await.unwrap();
    }
    let result = result.unwrap().unwrap();
    let attempts = conflicts.lock().unwrap().1.len();
    if release {
        assert_eq!(result, Err(CommandLedgerError::IdempotencyConflict));
        assert!(attempts >= 6, "observed only {attempts} conflicts");
    } else {
        assert_eq!(result, Err(CommandLedgerError::Conflict));
        if expires {
            assert!(started.elapsed() >= Duration::from_millis(200));
            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(
                store.reserve_command(retry).await,
                Err(CommandLedgerError::Conflict)
            );
        } else {
            assert!(attempts >= 6, "observed only {attempts} conflicts");
            assert!(started.elapsed() >= Duration::from_secs(4));
            assert!(started.elapsed() < Duration::from_secs(9));
            assert_eq!(
                store.reserve_command(retry).await,
                Err(CommandLedgerError::IdempotencyConflict)
            );
        }
    }
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_external_event_command(&key).await.unwrap(),
        Some(accepted.clone())
    );
    assert_eq!(
        reopened.reserve_command(first).await.unwrap(),
        CommandReservationResult::Existing(Box::new(accepted))
    );
    assert_eq!(
        store.agent_commands.count_documents(doc! {}).await.unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_event_duplicate_survives_six_real_write_conflicts() {
    contended(true, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_event_persistent_contention_is_bounded_and_retry_recovers() {
    contended(false, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn binding_event_contention_preserves_original_authority_expiry() {
    contended(false, true).await;
}
