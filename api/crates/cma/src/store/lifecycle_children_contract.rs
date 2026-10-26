//! Crash-window and concurrent admissions using the restricted runtime credential.
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::{profiles::*, resource_access::ResourceActor, sandbox::store::*, sandbox::*, *};
use mongodb::{
    bson::{doc, Bson, Document},
    Client,
};
use std::sync::Arc;

const NOW: &str = "2026-10-13T00:00:00Z";

pub(super) async fn verify(client: &Client, database: &str) {
    let store = Arc::new(MongoSandboxStore::open(client, database).await.unwrap());
    let origin = SandboxOrigin::Lifecycle {
        operation_id: "operation-child-replay".into(),
    };
    let mut sandbox =
        crate::store::sandbox_admission_mongo_integration_tests::sandbox("sbx_child", "unused");
    sandbox.origin = origin.clone();
    let owner = sandbox.owner.clone();
    let first = SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Allocate("trigger-research".into()),
        10,
        NOW.into(),
    )
    .unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let store = store.clone();
        let mut attempt = first.clone();
        attempt.sandbox.id = SandboxId::generate();
        tasks.spawn(async move { store.admit_sandbox(attempt).await.unwrap() });
    }
    let mut children = std::collections::BTreeSet::new();
    while let Some(result) = tasks.join_next().await {
        let sandbox = match result.unwrap() {
            SandboxAdmissionResult::Created(value) | SandboxAdmissionResult::Existing(value) => {
                value
            }
        };
        children.insert(sandbox.id);
    }
    assert_eq!(children.len(), 1);
    let sandbox_id = children.pop_first().unwrap();
    drop(store);
    let reopened = Arc::new(MongoSandboxStore::open(client, database).await.unwrap());
    let mut retry = first.clone();
    retry.sandbox.id = SandboxId::generate();
    retry.live_limit = 0;
    let SandboxAdmissionResult::Existing(mut recovered) =
        reopened.admit_sandbox(retry).await.unwrap()
    else {
        panic!("must recover committed Sandbox")
    };
    assert_eq!(recovered.id, sandbox_id);
    assert_eq!(
        reopened
            .sandbox_events(&sandbox_id, 0, 20)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(reopened
        .sandbox_admission_replay(&SubjectId::new("foreign-owner"), &origin)
        .await
        .unwrap()
        .is_none());

    let mut trigger = first.clone();
    trigger.sandbox.id = SandboxId::generate();
    trigger.sandbox.origin = SandboxOrigin::Trigger {
        launch_id: "operation-child-replay".into(),
    };
    let trigger = SandboxAdmission::new(trigger.sandbox, trigger.name, 10, NOW.into()).unwrap();
    let SandboxAdmissionResult::Created(other) = reopened.admit_sandbox(trigger).await.unwrap()
    else {
        panic!("Trigger has an independent origin namespace")
    };
    assert_ne!(other.id, sandbox_id);

    let rows = client
        .database(database)
        .collection::<Document>("sandboxes");
    let document = rows
        .find_one(doc! {"_id": sandbox_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        document.get_str("origin_operation_id").unwrap(),
        "operation-child-replay"
    );
    assert_eq!(document.get("origin_launch_id"), Some(&Bson::Null));
    rows.update_one(
        doc! {"_id": sandbox_id.as_str()},
        doc! {"$set": {"origin_operation_id":"tampered"}},
    )
    .await
    .unwrap();
    assert!(
        reopened
            .sandbox_admission_replay(&owner, &origin)
            .await
            .is_err(),
        "embedded origin discovers and rejects a tampered query field"
    );
    rows.update_one(
        doc! {"_id": sandbox_id.as_str()},
        doc! {"$set": {"origin_operation_id":"operation-child-replay"}},
    )
    .await
    .unwrap();

    // Advance the real child and verify replay metadata survives normal writes.
    recovered.status = SandboxStatus::Active;
    recovered.work = SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into());
    recovered.credential.sign_in = SandboxSignInState::Complete;
    recovered.revision += 1;
    reopened.save_sandbox(1, recovered.clone()).await.unwrap();
    assert_eq!(
        reopened
            .sandbox_admission_replay(&owner, &origin)
            .await
            .unwrap()
            .unwrap()
            .sandbox,
        recovered
    );
    verify_agent(client, database, reopened, recovered).await;
}

async fn verify_agent(
    client: &Client,
    database: &str,
    store: Arc<MongoSandboxStore>,
    parent: Sandbox,
) {
    let content = WorkspaceProfileContent {
        location: WorkspaceLocation::Directory {
            name: "workspace".into(),
        },
        name: "workspace".into(),
        setup_commands: vec![],
        agent_profiles: vec![],
    };
    let workspace = Workspace {
        id: WorkspaceId::generate(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: WorkspaceProfileSnapshot {
            id: WorkspaceProfileId("wkp_lifecycle_child".into()),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            content,
        },
        name: "workspace".into(),
        path: "/home/cma/workspace".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("a".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 1,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    };
    store.create_workspace(workspace.clone()).await.unwrap();
    let content = AgentProfileContent {
        isolated_agent: false,
        model: None,
        thinking_effort: ThinkingEffort::Medium,
        permission_level: PermissionLevel::Guarded,
        skills: vec![],
        system_prompt: String::new(),
    };
    let actor = ResourceActor::Api {
        subject: parent.owner.clone(),
        delegation_id: Some("captured-original-grant".into()),
    };
    let origin = AgentOrigin::Lifecycle {
        operation_id: "operation-child-replay".into(),
    };
    let first = AgentAdmission {
        binding: None,
        created_by: Some(actor.clone()),
        owner: parent.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: AgentId::generate(),
        worktree_id: WorktreeId::generate(),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_lifecycle_child".into()),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            content,
        },
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        title: Some("Research".into()),
        first_message: None,
        creation_key: format!("lifecycle:{}", content_digest(&origin).unwrap()),
        origin,
        event_at: NOW.into(),
    };
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let store = store.clone();
        let mut attempt = first.clone();
        attempt.agent_id = AgentId::generate();
        attempt.worktree_id = WorktreeId::generate();
        tasks.spawn(async move {
            store
                .admit_agent(attempt)
                .await
                .unwrap()
                .creation()
                .agent
                .clone()
        });
    }
    let mut children = std::collections::BTreeSet::new();
    while let Some(result) = tasks.join_next().await {
        children.insert(result.unwrap().id);
    }
    assert_eq!(children.len(), 1);
    let agent_id = children.pop_first().unwrap();
    drop(store);
    let reopened = MongoSandboxStore::open(client, database).await.unwrap();
    let replay = reopened.admit_agent(first.clone()).await.unwrap();
    assert_eq!(replay.creation().agent.id, agent_id);
    assert_eq!(
        reopened
            .list_worktrees(&workspace.id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(reopened.agent_log(&agent_id, 0, 20).await.unwrap().len(), 1);
    let provenance = reopened
        .agent_provenance_snapshot(&agent_id)
        .await
        .unwrap()
        .creation_provenance(&agent_id)
        .unwrap();
    assert_eq!(provenance.created_by, Some(actor));
    assert_eq!(provenance.creation_origin, AgentCreationOrigin::Manual);
    assert_eq!(provenance.provisioning_origin, first.origin);
    let mut changed = first;
    changed.title = Some("conflicting request".into());
    assert!(matches!(
        reopened.admit_agent(changed).await,
        Err(SandboxStoreError::AgentCreationConflict)
    ));
}
