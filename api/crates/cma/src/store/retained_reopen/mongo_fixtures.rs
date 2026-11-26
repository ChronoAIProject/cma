//! Real-Mongo recovery fixtures use complete creation/publication history.
use crate::store::{
    agent_log::log_document, agents::agent_document, sandbox_mongo::MongoSandboxStore,
    sandbox_unpublished_recovery::mongo_integration_tests::fixture,
};
use layer2_core::sandbox::{
    store::{SandboxStore, WorkspaceStore, WorktreeStore},
    *,
};
use mongodb::bson::doc;

const NOW: &str = "2026-09-05T00:30:00Z";

pub(in crate::store) async fn reopen_fixture(
    store: &MongoSandboxStore,
    id: &str,
) -> RetainedReopenRequest {
    let original = fixture(store, id).await;
    let mut parent = original.snapshot.sandbox;
    parent.runtime.as_mut().unwrap().runtime_version =
        format!("registry.example/machine@sha256:{}", "a".repeat(64));
    parent.profile.content.runtime_image =
        Some(parent.runtime.as_ref().unwrap().runtime_version.clone());
    parent.credential.delivered = true;
    parent.revision += 1;
    store
        .sandboxes
        .replace_one(
            doc! {"_id": parent.id.as_str()},
            crate::store::sandboxes::sandbox_document(&parent, 0).unwrap(),
        )
        .await
        .unwrap();
    let mut child = original.snapshot.agents[0].clone();
    child.conversation.queued_messages.clear();
    child.conversation.last_seq = 3;
    let workspace = Workspace {
        id: child.workspace_id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        origin: WorkspaceOrigin::Manual,
        profile: layer2_core::profiles::WorkspaceProfileSnapshot {
            id: layer2_core::WorkspaceProfileId("wkp_retained".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: layer2_core::profiles::WorkspaceProfileContent {
                location: layer2_core::profiles::WorkspaceLocation::Directory {
                    name: "project".into(),
                },
                name: "project".into(),
                setup_commands: vec![],
                agent_profiles: vec![],
            },
        },
        name: "project".into(),
        path: "/home/cma/project".into(),
        source: WorkspaceSource::Empty,
        resolved_commit: Some("c".repeat(40)),
        status: WorkspaceStatus::Ready,
        work: WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into()),
        next_worktree_number: 2,
        created_at: NOW.into(),
        ready_at: Some(NOW.into()),
        failed: None,
        removed_at: None,
        revision: 1,
    };
    let tree = Worktree {
        id: child.worktree_id.clone(),
        workspace_id: child.workspace_id.clone(),
        sandbox_id: parent.id.clone(),
        owner: parent.owner.clone(),
        name: "wt-1".into(),
        path: "/home/cma/project/.worktrees/wt-1".into(),
        branch: "cma/wt-1".into(),
        base_commit: "c".repeat(40),
        status: WorktreeStatus::Ready,
        agent_id: Some(child.id.clone()),
        revision: 1,
    };

    store.create_workspace(workspace).await.unwrap();
    store.create_worktree(tree).await.unwrap();
    store
        .agents
        .replace_one(
            doc! {"_id": child.id.as_str()},
            agent_document(&child).unwrap(),
        )
        .await
        .unwrap();
    store
        .agent_log
        .delete_many(doc! {"agent_id": child.id.as_str()})
        .await
        .unwrap();
    for (i, kind) in [
        AgentLogKind::Created {
            workspace_id: child.workspace_id.clone(),
            worktree_id: child.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: child.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::StatusChanged {
            from: AgentStatus::Ready,
            to: AgentStatus::Paused,
        },
    ]
    .into_iter()
    .enumerate()
    {
        store
            .agent_log
            .insert_one(
                log_document(&AgentLogEntry {
                    agent_id: child.id.clone(),
                    seq: i as u64 + 1,
                    appended_at: NOW.into(),
                    kind,
                })
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let snapshot = store.retained_reopen_snapshot(&parent.id).await.unwrap();
    snapshot.validate().unwrap();
    let provider_plan = serde_json::json!({"version": 1, "idleReopen": true});
    let proof = RetainedReopenProof {
        expected_runtime: parent.runtime.clone().unwrap(),
        observed_boot: "c".repeat(32),
        home_identity: snapshot.retained.source().unwrap().home_identity,
        provider_plan_sha256: SandboxUnpublishedRecoveryProof::provider_plan_digest(&provider_plan)
            .unwrap(),
        provider_plan,
        histories: vec![layer3_adapter::RecoveryConversationHistory {
            agent_id: child.id.to_string(),
            conversation_id: child.conversation.conversation_id.clone().unwrap(),
            history_sha256: "d".repeat(64),
        }],
    };
    RetainedReopenRequest {
        repeat_authorized: false,
        snapshot,
        proof,
        key_sha256: "e".repeat(64),
        requested_at: NOW.into(),
    }
}
