use std::sync::Arc;

use layer2_core::sandbox::store::{
    SandboxLifecycleCommand, SandboxLifecycleCommandDisposition, SandboxLifecycleCommandResult,
    SandboxLifecycleCommandStore,
};
use layer2_core::sandbox::{
    SandboxCheckpointConversation, SandboxCheckpointRef, SandboxLifecycleOperation,
    SandboxReviveCause,
};
use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lifecycle_commands_are_owner_bound_gap_free_and_concurrently_idempotent() {
    let (client, name) = database("lifecycle_command").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let active = sandbox("sbx_lifecycle_command");
    store.create_sandbox(active.clone()).await.unwrap();

    let results = futures_util::future::join_all((0..16).map(|_| {
        let store = store.clone();
        let active = active.clone();
        async move {
            store
                .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                    active.id,
                    active.owner,
                    NOW.into(),
                    None,
                ))
                .await
        }
    }))
    .await;
    let mut scheduled = 0;
    let mut replayed = 0;
    for result in results {
        let SandboxLifecycleCommandResult::Accepted(outcome) = result.unwrap() else {
            panic!("eligible pause was refused");
        };
        match outcome.disposition {
            SandboxLifecycleCommandDisposition::Scheduled => scheduled += 1,
            SandboxLifecycleCommandDisposition::AlreadyScheduled => replayed += 1,
            SandboxLifecycleCommandDisposition::AlreadySatisfied => {
                panic!("no worker completed the pause")
            }
        }
    }
    assert_eq!((scheduled, replayed), (1, 15));
    let stored = store.get_sandbox(&active.id).await.unwrap().unwrap();
    assert_eq!(stored.revision, 2);
    assert_eq!(stored.status, SandboxStatus::Pausing);
    assert_eq!(stored.lifecycle.next_authority, 2);
    assert!(matches!(
        stored.lifecycle.operation,
        Some(SandboxLifecycleOperation::Pause { .. })
    ));
    let events = store.sandbox_events(&active.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 1);
    assert_eq!(
        store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                active.id.clone(),
                SubjectId("stranger".into()),
                MINUTE.into(),
                None,
            ))
            .await,
        Err(SandboxStoreError::NotFound)
    );

    let mut paused = sandbox("sbx_lifecycle_revive");
    paused.name = "lifecycle-revive".into();
    paused.status = SandboxStatus::Paused;
    paused.runtime = None;
    paused.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    paused.lifecycle.checkpoints.latest = Some(checkpoint(7));
    store.create_sandbox(paused.clone()).await.unwrap();
    let SandboxLifecycleCommandResult::Accepted(revived) = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
            paused.id.clone(),
            paused.owner.clone(),
            MINUTE.into(),
            SandboxReviveCause::ExplicitRequest,
        ))
        .await
        .unwrap()
    else {
        panic!("eligible revive was refused");
    };
    assert_eq!(
        revived.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert!(matches!(
        revived.sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            checkpoint_epoch: 7,
            ..
        })
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn lifecycle_command_reconciles_an_ambiguous_transaction_commit_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-sandbox-lifecycle-{suffix}");
    let database = format!("cma_812_lifecycle_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &database).await.unwrap();
    let active = sandbox("sbx_lifecycle_ambiguous");
    store.create_sandbox(active.clone()).await.unwrap();

    admin
        .database("admin")
        .run_command(doc! {
            "configureFailPoint": "failCommand",
            "mode": { "times": 5_i32 },
            "data": {
                "failCommands": ["commitTransaction"],
                "appName": &app_name,
                "writeConcernError": {
                    "code": 50_i32,
                    "codeName": "MaxTimeMSExpired",
                    "errmsg": "injected ambiguous lifecycle commit",
                },
            },
        })
        .await
        .unwrap();
    let result = store
        .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
            active.id.clone(),
            active.owner.clone(),
            NOW.into(),
            None,
        ))
        .await;
    admin
        .database("admin")
        .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
        .await
        .expect("commit failpoint must be disabled before assertions");

    let SandboxLifecycleCommandResult::Accepted(outcome) = result.unwrap() else {
        panic!("ambiguous commit did not reconcile");
    };
    assert_eq!(
        outcome.disposition,
        SandboxLifecycleCommandDisposition::Scheduled
    );
    assert_eq!(
        store
            .get_sandbox(&active.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        store.sandbox_events(&active.id, 0, 10).await.unwrap().len(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

fn checkpoint(epoch: u64) -> SandboxCheckpointRef {
    let archive = b"durable conversation archive".to_vec();
    let capture = SandboxCheckpointConversation::new(
        AgentId("agt_checkpoint".into()),
        "conversation-checkpoint",
        ConversationSnapshot {
            conversation_id: "conversation-checkpoint".into(),
            archive_sha256: format!("{:x}", Sha256::digest(&archive)),
            archive,
        },
    )
    .unwrap();
    SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch,
            object_reference: format!("checkpoints/{epoch}"),
            sha256: "b".repeat(64),
            writers: WriterState::Held,
        },
        &[capture],
        NOW,
    )
    .unwrap()
}
