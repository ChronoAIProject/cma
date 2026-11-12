use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn writer_caller_unknown_admission_keeps_original_identity_and_retry_adopts_once() {
    let (admin, name, _, current) = legacy("caller_admit_unknown").await;
    gate(&admin, &name, &current, true, true).await;
    let (client, app) = super::super::uncertain::client_for("caller_admit").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mutation = output(&current, "original admission");
    let before = snapshot_raw(&store).await;
    super::super::uncertain::fail_commit(&admin, &app, true).await;
    let result = store.commit_agent_conversation(mutation.clone()).await;
    super::super::uncertain::clear(&admin).await;
    assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
    let after = snapshot_raw(&store).await;
    assert_eq!(after["agents"], before["agents"]);
    assert_eq!(after["agent_log"], before["agent_log"]);
    assert_eq!(after[segments::writer_admission::COLLECTION].len(), 1);
    assert!(after[segments::PUBLICATIONS].is_empty());
    let admitted = after[segments::writer_admission::COLLECTION].clone();
    assert_eq!(
        store
            .commit_agent_conversation(mutation.clone())
            .await
            .unwrap(),
        conversation::conversation_logs(&mutation).unwrap()
    );
    let final_state = snapshot_raw(&store).await;
    assert_eq!(
        final_state[segments::writer_admission::COLLECTION],
        admitted
    );
    assert_eq!(
        final_state["agent_log"].len(),
        before["agent_log"].len() + 1
    );
    assert_eq!(version(&store, &current).await, 2);
    admin.database(&name).drop().await.unwrap();
}

/// Arm ambiguity only when the real root sends its v1 -> v2 Agent CAS.
/// Earlier admission/staging commits remain unmodified. The callback waits for
/// the isolated admin connection, so no timing/sleep guesses select the phase.
async fn publication_fault_client(
    committed: bool,
) -> (Client, Arc<AtomicBool>, Arc<Mutex<Option<Document>>>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app = format!("writer-caller-final-{}", uuid::Uuid::new_v4());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let fired = Arc::new(AtomicBool::new(false));
    let armed = fired.clone();
    let original = Arc::new(Mutex::new(None));
    let observed = original.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Started(event) = event else {
            return;
        };
        if event.command_name == "commitTransaction" && armed.load(Ordering::SeqCst) {
            *observed.lock().unwrap() = Some(event.command.get_document("lsid").unwrap().clone());
        }
        let adopts = event.command.get_str("update").ok() == Some("agents")
            && event.command.get_array("updates").is_ok_and(|updates| {
                updates.iter().any(|update| {
                    update
                        .as_document()
                        .and_then(|u| u.get_document("u").ok())
                        .and_then(|u| u.get_document("$set").ok())
                        .is_some_and(|fields| {
                            fields.get_i64("document_schema_version").ok() == Some(2)
                        })
                })
            });
        if !adopts || armed.swap(true, Ordering::SeqCst) {
            return;
        }
        let uri = uri.clone();
        let app = app.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let admin = Client::with_uri_str(&uri).await.unwrap();
                    super::super::uncertain::fail_commit(&admin, &app, committed).await;
                });
        })
        .join()
        .unwrap();
    }));
    (Client::with_options(options).unwrap(), fired, original)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn writer_caller_unknown_publication_reconciles_exact_adoption_receipt_or_refuses() {
    for committed in [true, false] {
        let (admin, name, _, current) = legacy("caller_final_unknown").await;
        gate(&admin, &name, &current, true, true).await;
        let (client, fired, original) = publication_fault_client(committed).await;
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let mutation = output(&current, "original final publication");
        let result = store.commit_agent_conversation(mutation.clone()).await;
        super::super::uncertain::clear(&admin).await;
        assert!(
            fired.load(Ordering::SeqCst),
            "fault must cover adoption publication"
        );
        if committed {
            assert_eq!(
                result.unwrap(),
                conversation::conversation_logs(&mutation).unwrap()
            );
            assert_eq!(version(&store, &current).await, 2);
            let rows = snapshot_raw(&store).await;
            assert_eq!(rows[segments::PUBLICATIONS].len(), 1);
            assert_eq!(
                store.get_agent(&current.id).await.unwrap(),
                Some(mutation.agent.clone())
            );
            assert!(matches!(
                store.commit_agent_conversation(mutation).await,
                Err(SandboxStoreError::Conflict)
            ));
            assert_eq!(snapshot_raw(&store).await, rows);
        } else {
            assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
            super::super::uncertain::release_abandoned_transaction(&admin, &original).await;
            assert_eq!(version(&store, &current).await, 1);
            let rows = snapshot_raw(&store).await;
            assert!(rows[segments::PUBLICATIONS].is_empty());
            assert_eq!(store.get_agent(&current.id).await.unwrap(), Some(current));
        }
        admin.database(&name).drop().await.unwrap();
    }
}
