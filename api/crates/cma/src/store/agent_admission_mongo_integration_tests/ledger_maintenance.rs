use super::*;
use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
use mongodb::{bson::Bson, options::WriteConcern};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_fences_implicit_profile_reuse_after_the_parent_read() {
    let (client, database, store, parent, mut workspace, mut automatic) =
        fixture("ledger_hold_profile").await;
    workspace.profile.content.agent_profiles = vec![layer2_core::profiles::AgentProfileRef::new(
        automatic.profile.id.clone(),
        automatic.profile.revision,
    )];
    workspace.profile.digest =
        layer2_core::profiles::content_digest(&workspace.profile.content).unwrap();
    let revision = workspace.revision;
    workspace.revision += 1;
    store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    automatic.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    automatic.creation_key = format!("workspace-profile:{}:0", workspace.id);
    let mut trigger = automatic.clone();
    trigger.agent_id = AgentId::generate();
    trigger.worktree_id = WorktreeId::generate();
    trigger.origin = AgentOrigin::Trigger {
        trigger_id: "trg_profile_reuse".into(),
        launch_id: "lch_profile_reuse".into(),
    };
    trigger.creation_key = format!(
        "trigger-profile:{}",
        layer2_core::profiles::content_digest(&trigger.origin).unwrap()
    );
    trigger.first_message = Some(
        layer2_core::sandbox::FirstMessageRecord::new(
            trigger.creation_key.clone(),
            "Introduce the repository.",
            NOW,
        )
        .unwrap(),
    );
    let created = store
        .admit_agent_reusing_profile(trigger.clone())
        .await
        .unwrap();
    let agent_filter = doc! {"_id": created.creation().agent.id.as_str()};
    let workspace_filter = doc! {"_id": workspace.id.as_str()};
    let original_agent = store.agents.find_one(agent_filter.clone()).await.unwrap();
    let original_workspace = store
        .workspaces
        .find_one(workspace_filter.clone())
        .await
        .unwrap();

    let observed = Arc::new(tokio::sync::Notify::new());
    let ready = observed.clone();
    let armed = Arc::new(AtomicBool::new(false));
    let gate = armed.clone();
    let (resume, released) = mpsc::sync_channel(1);
    let released = Mutex::new(released);
    let namespace = format!("{database}.{}", store.sandboxes.name());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Succeeded(event) = event else {
            return;
        };
        if event.command_name == "find"
            && event.reply.get_document("cursor").is_ok_and(|cursor| {
                cursor.get_str("ns").ok() == Some(namespace.as_str())
                    && cursor
                        .get_array("firstBatch")
                        .is_ok_and(|rows| !rows.is_empty())
            })
            && gate.swap(false, Ordering::SeqCst)
        {
            ready.notify_one();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
    }));
    let monitored = MongoSandboxStore::open(&Client::with_options(options).unwrap(), &database)
        .await
        .unwrap();
    armed.store(true, Ordering::SeqCst);
    let request = automatic.clone();
    let pending = tokio::spawn(async move { monitored.admit_agent(request).await });
    tokio::time::timeout(Duration::from_secs(5), observed.notified())
        .await
        .unwrap();
    let filter = doc! {"_id": parent.id.as_str()};
    store
        .sandboxes
        .update_one(
            filter.clone(),
            doc! {"$set": {LEDGER_MAINTENANCE_FIELD: null}},
        )
        .write_concern(WriteConcern::majority())
        .await
        .unwrap();
    let held_parent = store
        .sandboxes
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held_parent.get(LEDGER_MAINTENANCE_FIELD), Some(&Bson::Null));
    resume.send(()).unwrap();
    assert_eq!(
        pending.await.unwrap(),
        Err(SandboxStoreError::SandboxNotReady)
    );
    let replay = store.admit_agent_reusing_profile(trigger).await.unwrap();
    assert_eq!(replay.creation(), created.creation());
    assert_eq!(
        store.sandboxes.find_one(filter.clone()).await.unwrap(),
        Some(held_parent)
    );
    assert_eq!(
        store.agents.find_one(agent_filter).await.unwrap(),
        original_agent
    );
    assert_eq!(
        store.workspaces.find_one(workspace_filter).await.unwrap(),
        original_workspace
    );
    assert!(store
        .get_agent(&automatic.agent_id)
        .await
        .unwrap()
        .is_none());

    store
        .sandboxes
        .update_one(filter, doc! {"$unset": {LEDGER_MAINTENANCE_FIELD: ""}})
        .await
        .unwrap();
    let reuse = store.admit_agent(automatic).await.unwrap();
    assert!(matches!(reuse, AgentAdmissionResult::Existing(_)));
    assert_eq!(reuse.creation().agent.id, created.creation().agent.id);
    client.database(&database).drop().await.unwrap();
}
