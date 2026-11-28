use super::*;
use layer2_core::sandbox::{
    AgentDescriptionSource, AgentMessageAssembly, AgentRuntimeUpdate, AgentUpdateRecorder,
    DurableAgentUpdateRecorder,
};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn runtime_loss_rebuilds_two_agent_publication_after_peer_log_advances() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let name = format!("cma_runtime_overlap_{}", uuid::Uuid::new_v4().simple());
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(name.clone());
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                observed.fetch_add(1, Ordering::SeqCst);
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let other = Arc::new(MongoSandboxStore::open(&admin, &name).await.unwrap());
    let (mut parent, first) = fixture(&store, "sbx_runtime_overlap").await;
    let second = ready_agent(&parent, "agt_runtime_overlap_peer");
    insert_agent(&store, &second, BEFORE).await;
    parent.lifecycle.checkpoints.latest = Some(checkpoint(&[first.clone(), second.clone()]));
    let baseline = parent.revision;
    parent.revision += 1;
    store.save_sandbox(baseline, parent.clone()).await.unwrap();
    attempts.store(0, Ordering::SeqCst);
    let admin_db = admin.database("admin");
    let armed = admin_db
        .run_command(doc! {
            "configureFailPoint": "failCommand", "mode": {"times": 1_i32},
            "data": {"failCommands": ["update"], "appName": &name,
                "namespace": format!("{name}.{}", store.agents.name()),
                "blockConnection": true, "blockTimeMS": 3500_i32},
        })
        .await
        .unwrap();
    let pending = store.recover_runtime_loss(loss(&parent));
    let raced = async {
        let count = armed
            .get_i64("count")
            .or_else(|_| armed.get_i32("count").map(i64::from))
            .unwrap();
        admin_db
            .run_command(
                doc! {"waitForFailPoint": "failCommand", "timesEntered": count + 1,
                "maxTimeMS": 6000_i32},
            )
            .await
            .unwrap();
        assert_eq!(
            other.get_sandbox(&parent.id).await.unwrap(),
            Some(parent.clone())
        );
        assert_eq!(
            other.get_agent(&second.id).await.unwrap(),
            Some(second.clone())
        );
        let mut update = AgentRuntimeUpdate::Metadata(Default::default());
        if let AgentRuntimeUpdate::Metadata(metadata) = &mut update {
            metadata.description = Some("Peer metadata before runtime recovery".into());
            metadata.description_source = Some(AgentDescriptionSource::ProviderTitle);
        }
        DurableAgentUpdateRecorder::new(other.clone())
            .record_update(
                &first.id,
                1,
                &mut AgentMessageAssembly::default(),
                update,
                OBSERVED.into(),
            )
            .await
            .unwrap();
    };
    let ((), recovered) = tokio::join!(
        raced,
        tokio::time::timeout(Duration::from_secs(20), pending)
    );
    disable_failpoint(&admin).await;
    assert_eq!(
        recovered.unwrap().unwrap(),
        SandboxRuntimeLossOutcome::RecoveryScheduled
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let settled = other.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert_eq!(settled.revision, parent.revision + 1);
    assert!(settled.runtime.is_none());
    for before in [first.clone(), second] {
        let after = other.get_agent(&before.id).await.unwrap().unwrap();
        assert_eq!(after.status, AgentStatus::Paused);
        assert_eq!(
            after.conversation.conversation_id,
            before.conversation.conversation_id
        );
        assert_eq!(after.conversation.runtime_generation, Some(2));
        let logs = other.agent_log(&before.id, 0, 100).await.unwrap();
        assert_eq!(
            logs.iter()
                .filter(|entry| matches!(
                    entry.kind,
                    AgentLogKind::RuntimeGenerationRetired {
                        previous_generation: 1,
                        runtime_generation: 2
                    }
                ))
                .count(),
            1
        );
        if before.id == first.id {
            assert_eq!(
                after
                    .conversation
                    .observations
                    .metadata
                    .description
                    .as_deref(),
                Some("Peer metadata before runtime recovery")
            );
        }
    }
    client.database(&name).drop().await.unwrap();
}
