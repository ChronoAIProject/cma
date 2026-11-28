use mongodb::{options::ClientOptions, Client};

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn checkpoint_paces_a_proven_abort_and_keeps_the_exact_receipt() {
    use mongodb::{event::command::CommandEvent, event::EventHandler};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!("cma_checkpoint_pacing_{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    let commands = Arc::new(Mutex::new((None, None)));
    let observed = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let mut commands = observed.lock().unwrap();
        match event {
            CommandEvent::Failed(event) if event.command_name == "update" => {
                commands.0 = Some(Instant::now());
            }
            CommandEvent::Started(event)
                if event
                    .command
                    .get_bool("startTransaction")
                    .is_ok_and(|start| start)
                    && commands.0.is_some() =>
            {
                commands.1.get_or_insert_with(Instant::now);
            }
            _ => {}
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let current = working(&store, "sbx_checkpoint_pacing").await;
    let checkpoint = partial(&current, "retained partial");
    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 1_i32 },
            "data": { "failCommands": ["update"], "appName": &name,
                      "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"] },
        })
        .await
        .unwrap();
    let result = store
        .commit_agent_message_update(current.clone(), None, checkpoint.clone(), None)
        .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    let (failure, retry) = *commands.lock().unwrap();
    let retry_delay = retry.unwrap().duration_since(failure.unwrap());
    assert!(retry_delay >= Duration::from_millis(10), "{retry_delay:?}");
    assert_eq!(result.unwrap(), checkpoint.outcome);
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(checkpoint),
    );
    assert_eq!(store.get_agent(&current.id).await.unwrap(), Some(current),);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn checkpoint_reconciles_ambiguous_quiet_and_boundary_commits_without_duplicate_logs() {
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    let _metrics = metrics::set_default_local_recorder(&recorder);
    let terminal_count = || -> f64 {
        handle
            .render()
            .lines()
            .filter(|line| line.starts_with("cma_terminal_publications_total{"))
            .map(|line| {
                line.split_whitespace()
                    .last()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
            })
            .sum()
    };
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-message-checkpoint-{suffix}");
    let name = format!("cma_message_checkpoint_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let current = working(&store, "sbx_checkpoint_ambiguous").await;
    let mut previous = None;
    for quiet in [true, false] {
        let mutation = (!quiet).then(|| {
            let mut mutation = update(&current, "prefix and final suffix");
            mutation.agent.conversation.active_turn = None;
            mutation.agent.status = AgentStatus::Ready;
            mutation.agent_logs.push(AgentLogKind::TurnCompleted {
                turn_sequence: current.conversation.active_turn.unwrap(),
            });
            mutation.agent.conversation.last_seq += 1;
            mutation
        });
        let mut checkpoint = mutation
            .as_ref()
            .map(boundary)
            .unwrap_or_else(|| partial(&current, "prefix"));
        checkpoint.outcome.turn_closed = !quiet;
        admin.database("admin").run_command(doc! {
            "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
            "data": { "failCommands": ["commitTransaction"], "appName": &app_name,
                "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous message checkpoint" } },
        }).await.unwrap();
        let result = store
            .commit_agent_message_update(
                current.clone(),
                previous.clone(),
                checkpoint.clone(),
                mutation.clone(),
            )
            .await;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .expect("disable failpoint before assertions");
        assert_eq!(result.unwrap(), checkpoint.outcome);
        let before_replay = terminal_count();
        assert_eq!(before_replay, if quiet { 0. } else { 1. });
        assert_eq!(
            store.agent_message_checkpoint(&current.id).await.unwrap(),
            Some(checkpoint.clone())
        );
        assert_eq!(
            store
                .commit_agent_message_update(
                    current.clone(),
                    previous,
                    checkpoint.clone(),
                    mutation.clone()
                )
                .await
                .unwrap(),
            checkpoint.outcome
        );
        assert_eq!(
            terminal_count(),
            before_replay,
            "retained checkpoint terminal was counted twice"
        );
        let expected_agent = mutation
            .map(|value| value.agent)
            .unwrap_or_else(|| current.clone());
        assert_eq!(
            store.get_agent(&current.id).await.unwrap(),
            Some(expected_agent.clone())
        );
        assert_eq!(
            store.agent_log(&current.id, 0, 100).await.unwrap().len(),
            expected_agent.conversation.last_seq as usize
        );
        previous = Some(checkpoint);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn checkpoint_reanchor_reconciles_ambiguous_commit_without_replaying_the_old_anchor() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-checkpoint-reanchor-{suffix}");
    let name = format!("cma_checkpoint_reanchor_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let current = super::anchor::working_with_worktree(&store, "sbx_checkpoint_reanchor").await;
    let previous = partial(&current, "retained answer");
    store
        .commit_agent_message_update(current.clone(), None, previous.clone(), None)
        .await
        .unwrap();
    super::anchor::publish_legacy_connection(&store, &current.id).await;
    let current = store.get_agent(&current.id).await.unwrap().unwrap();
    let next = previous.reanchored(&current).unwrap();
    // The old exact receipt proves the original chunk, not the requested repair.
    assert_eq!(
        checkpoint::replay_at_anchor(Some(&previous), &next, true).unwrap(),
        None
    );
    assert_eq!(
        checkpoint::replay_at_anchor(Some(&previous), &next, false).unwrap(),
        Some(previous.outcome.clone())
    );
    let logs = store.agent_log(&current.id, 0, 200).await.unwrap();
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": { "times": 5_i32 },
        "data": { "failCommands": ["commitTransaction"], "appName": &app_name,
            "writeConcernError": { "code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "injected ambiguous checkpoint reanchor" } },
    }).await.unwrap();
    let result = store
        .commit_agent_message_update(current.clone(), Some(previous.clone()), next.clone(), None)
        .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .unwrap();
    assert_eq!(result.unwrap(), previous.outcome);
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(next.clone())
    );
    // Repeating the exact anchor CAS keeps the original receipt and no public writes.
    assert_eq!(
        store
            .commit_agent_message_update(current.clone(), Some(previous), next.clone(), None)
            .await
            .unwrap(),
        next.outcome
    );
    assert_eq!(
        store.get_agent(&current.id).await.unwrap(),
        Some(current.clone())
    );
    assert_eq!(store.agent_log(&current.id, 0, 200).await.unwrap(), logs);
    let mut conflicting = next.clone();
    conflicting.update_fingerprint = "b".repeat(64);
    assert!(matches!(
        store
            .commit_agent_message_update(current.clone(), Some(next.clone()), conflicting, None)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store.agent_message_checkpoint(&current.id).await.unwrap(),
        Some(next)
    );
    client.database(&name).drop().await.unwrap();
}
