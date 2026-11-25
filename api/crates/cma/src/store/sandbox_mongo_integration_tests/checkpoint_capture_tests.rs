use crate::store::sandbox_mongo_integration_tests::work_lease_deadline;
use layer2_core::sandbox::store::{
    SandboxLifecycleAgentScope, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxMutationGuard,
};

use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{
    active_sandbox, insert_agent, ready_agent,
};

mod concurrency_tests;
mod cooldown_tests;
mod identity_receipt_tests;
mod publication_tests;
mod quiet_tests;
mod renewal_tests;
mod writer_retry_tests;

const QUIET_AT: &str = "2026-09-04T23:58:00Z";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_capture_waits_for_missing_pinned_agents_in_the_same_transaction() {
    let (client, name) = database("capture_profile_priority").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut parent = active_sandbox("sbx_capture_profile_priority");
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    store.create_sandbox(parent.clone()).await.unwrap();
    let mut agent = ready_agent(&parent, "agt_capture_profile_priority");
    let mut workspace = ready_workspace(&parent, agent.workspace_id.as_str(), "project");
    workspace
        .profile
        .content
        .agent_profiles
        .push(layer2_core::profiles::AgentProfileRef {
            id: agent.profile.id.clone(),
            revision: agent.profile.revision,
        });
    agent.origin = layer2_core::sandbox::AgentOrigin::WorkspaceProfile { index: 0 };
    agent.creation_key = format!("workspace-profile:{}:0", workspace.id);
    store.create_workspace(workspace.clone()).await.unwrap();
    let request = begin(&parent);
    assert_eq!(
        store.commit_sandbox_lifecycle(request.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&parent.id).await.unwrap(),
        Some(parent.clone())
    );
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    assert!(store
        .sandbox_events(&parent.id, 0, 10)
        .await
        .unwrap()
        .is_empty());
    insert_agent(&store, &agent, QUIET_AT).await;
    store
        .commit_sandbox_lifecycle(request.clone())
        .await
        .unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&parent.id).await.unwrap(),
        Some(request.sandbox)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_writer_claims_require_persisted_parent_authority() {
    let (client, name) = database("writer_missing_parent").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let parent = active_sandbox("sbx_missing_writer_parent");
    let mut agent = ready_agent(&parent, "agt_missing_writer_parent");
    agent.work = AgentWorkState::new(AgentWorkStep::CloseProcess, NOW.into());
    insert_agent(&store, &agent, NOW).await;
    let workspace = workspace(&parent, agent.workspace_id.as_str(), "project");
    store.create_workspace(workspace.clone()).await.unwrap();
    assert_eq!(
        store
            .claim_agent_work(&agent.id, "writer", NOW.into(), MINUTE.into())
            .await,
        Err(SandboxStoreError::NotFound)
    );
    assert_eq!(
        store
            .claim_workspace_work(&workspace.id, "writer", NOW.into(), work_lease_deadline())
            .await,
        Err(SandboxStoreError::NotFound)
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_capture_begin_requires_every_agent_log_to_match_its_head() {
    let (client, name) = database("capture_log_integrity").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for missing in [true, false] {
        let (parent, _, agent) = capture_fixture(&store, &format!("log_{missing}")).await;
        if missing {
            store
                .agent_log
                .delete_one(doc! { "agent_id": agent.id.as_str(), "seq": 1_i64 })
                .await
                .unwrap();
        } else {
            let entry = AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: 2,
                appended_at: NOW.into(),
                kind: AgentLogKind::ConversationOpened {
                    conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                    runtime_generation: 1,
                },
            };
            store
                .agent_log
                .insert_one(crate::store::agent_log::log_document(&entry).unwrap())
                .await
                .unwrap();
        }
        assert_eq!(
            store.commit_sandbox_lifecycle(begin(&parent)).await,
            Err(SandboxStoreError::SequenceConflict)
        );
        assert_eq!(
            store.get_sandbox(&parent.id).await.unwrap(),
            Some(parent.clone())
        );
        assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
        assert!(store
            .sandbox_events(&parent.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }
    client.database(&name).drop().await.unwrap();
}

async fn capture_fixture(store: &MongoSandboxStore, suffix: &str) -> (Sandbox, Workspace, Agent) {
    let mut parent = active_sandbox(&format!("sbx_capture_{suffix}"));
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    store.create_sandbox(parent.clone()).await.unwrap();
    let agent = ready_agent(&parent, &format!("agt_capture_{suffix}"));
    let workspace = ready_workspace(&parent, agent.workspace_id.as_str(), "project");
    store.create_workspace(workspace.clone()).await.unwrap();
    insert_agent(store, &agent, QUIET_AT).await;
    (parent, workspace, agent)
}

fn mutation(current: &Sandbox, next: Sandbox, at: &str) -> SandboxLifecycleMutation {
    SandboxLifecycleMutation {
        guard: SandboxMutationGuard::capture(current),
        sandbox: next,
        agents: vec![],
        agent_scope: SandboxLifecycleAgentScope::Touched,
        appended_at: at.into(),
        events: vec![SandboxEventKind::WorkStepCompleted {
            step: SandboxWorkStep::RunLifecycleOperation,
        }],
    }
}

fn begin(current: &Sandbox) -> SandboxLifecycleMutation {
    let mut next = current.clone();
    next.begin_checkpoint_capture(NOW.into()).unwrap();
    mutation(current, next, NOW)
}

async fn advance_capture(store: &MongoSandboxStore, current: &mut Sandbox, next: Sandbox) {
    store
        .commit_sandbox_lifecycle(mutation(current, next.clone(), NOW))
        .await
        .unwrap();
    *current = next;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_capture_holds_every_writer_until_exact_resume_acknowledgement() {
    let (client, name) = database("capture_writer_debt").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, mut workspace, mut agent) = capture_fixture(&store, "writer_debt").await;
    let stale_input = begin_stored_conversation_turn(&store, agent.clone()).await;
    let request = begin(&parent);
    store
        .commit_sandbox_lifecycle(request.clone())
        .await
        .unwrap();
    let mut current = request.sandbox;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(current.status, SandboxStatus::Active);
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap(),
        Some(agent.clone())
    );
    assert_eq!(
        store.get_workspace(&workspace.id).await.unwrap(),
        Some(workspace.clone())
    );
    assert_eq!(
        store.commit_agent_conversation(stale_input).await,
        Err(SandboxStoreError::CheckpointInProgress)
    );
    let input = begin_stored_conversation_turn(&store, agent.clone()).await;
    assert_eq!(
        store.commit_agent_conversation(input).await,
        Err(SandboxStoreError::CheckpointInProgress)
    );
    assert_eq!(
        store
            .admit_agent(agent_admission(&workspace, "capture-new", "capture_new"))
            .await,
        Err(SandboxStoreError::SandboxNotReady)
    );
    assert_eq!(
        store
            .admit_workspace(workspace_admission(super::workspace(
                &current,
                "wks_capture_new",
                "new-project"
            )))
            .await,
        Err(SandboxStoreError::SandboxNotReady)
    );
    assert_eq!(
        store
            .begin_workspace_removal(&workspace.id, &parent.owner, NOW.into())
            .await,
        Err(SandboxStoreError::SandboxNotReady)
    );

    let revision = agent.revision;
    agent.work = AgentWorkState::new(AgentWorkStep::CloseProcess, NOW.into());
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    let revision = workspace.revision;
    workspace
        .transition(WorkspaceStatus::Removing, NOW.into(), None)
        .unwrap();
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Remove, NOW.into());
    store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    let revision = current.revision;
    current.work = SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into());
    current.revision += 1;
    store.save_sandbox(revision, current.clone()).await.unwrap();
    assert_no_writer_claims(&store, &current, &workspace, &agent).await;

    let mut claimed = current.clone();
    claimed
        .claim_checkpoint_capture("capture".into(), NOW.into(), MINUTE.into())
        .unwrap();
    advance_capture(&store, &mut current, claimed).await;
    assert_no_writer_claims(&store, &current, &workspace, &agent).await;
    let capture = current.lifecycle.checkpoint_capture.as_ref().unwrap();
    let token = capture.authority.writer_resume_token.clone();
    let fence = capture.work.lease.as_ref().unwrap().fence;
    let mut releasing = current.clone();
    releasing
        .require_checkpoint_writer_resume("capture", fence, NOW.into())
        .unwrap();
    advance_capture(&store, &mut current, releasing).await;
    assert_eq!(
        current
            .lifecycle
            .checkpoint_capture
            .as_ref()
            .unwrap()
            .authority
            .writer_resume_token,
        token
    );
    assert_no_writer_claims(&store, &current, &workspace, &agent).await;

    let mut claimed = current.clone();
    claimed
        .claim_checkpoint_capture("resume".into(), NOW.into(), MINUTE.into())
        .unwrap();
    advance_capture(&store, &mut current, claimed).await;
    let fence = current
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .work
        .lease
        .as_ref()
        .unwrap()
        .fence;
    let mut complete = current.clone();
    complete
        .complete_checkpoint_writer_resume("resume", fence, NOW.into())
        .unwrap();
    advance_capture(&store, &mut current, complete).await;
    assert!(current.lifecycle.checkpoint_capture.is_none());
    assert!(store
        .claim_sandbox_work(&current.id, "normal", NOW.into(), work_lease_deadline())
        .await
        .unwrap()
        .is_some());
    assert!(store
        .claim_workspace_work(&workspace.id, "normal", NOW.into(), work_lease_deadline())
        .await
        .unwrap()
        .is_some());
    assert!(store
        .claim_agent_work(&agent.id, "normal", NOW.into(), MINUTE.into())
        .await
        .unwrap()
        .is_some());
    client.database(&name).drop().await.unwrap();
}

async fn assert_no_writer_claims(
    store: &MongoSandboxStore,
    parent: &Sandbox,
    workspace: &Workspace,
    agent: &Agent,
) {
    assert!(store
        .claim_sandbox_work(
            &parent.id,
            "writer",
            TWO_MINUTES.into(),
            work_lease_deadline()
        )
        .await
        .unwrap()
        .is_none());
    assert!(store
        .claim_workspace_work(
            &workspace.id,
            "writer",
            TWO_MINUTES.into(),
            work_lease_deadline()
        )
        .await
        .unwrap()
        .is_none());
    assert!(store
        .claim_agent_work(
            &agent.id,
            "writer",
            TWO_MINUTES.into(),
            "2026-09-05T00:03:00Z".into()
        )
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn checkpoint_capture_begin_refuses_pending_and_expired_child_writer_work() {
    let (client, name) = database("capture_pending_writer").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    for (kind, expired) in [
        ("agent", false),
        ("agent", true),
        ("workspace", false),
        ("workspace", true),
    ] {
        let (parent, mut workspace, mut agent) =
            capture_fixture(&store, &format!("{kind}_{expired}")).await;
        if kind == "agent" {
            let revision = agent.revision;
            agent.work = AgentWorkState::new(AgentWorkStep::CloseProcess, NOW.into());
            if expired {
                agent.work.claim("old".into(), NOW, MINUTE.into()).unwrap();
            }
            agent.revision += 1;
            store.save_agent(revision, agent).await.unwrap();
        } else {
            let revision = workspace.revision;
            workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Remove, NOW.into());
            workspace.status = WorkspaceStatus::Removing;
            if expired {
                workspace
                    .work
                    .claim("old".into(), NOW, MINUTE.into())
                    .unwrap();
            }
            workspace.revision += 1;
            store.save_workspace(revision, workspace).await.unwrap();
        }
        let mut next = parent.clone();
        next.begin_checkpoint_capture(TWO_MINUTES.into()).unwrap();
        assert_eq!(
            store
                .commit_sandbox_lifecycle(mutation(&parent, next, TWO_MINUTES))
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store.get_sandbox(&parent.id).await.unwrap(),
            Some(parent.clone())
        );
        assert!(store
            .sandbox_events(&parent.id, 0, 10)
            .await
            .unwrap()
            .is_empty());
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn generic_sandbox_saves_cannot_create_or_erase_checkpoint_capture_debt() {
    let (client, name) = database("capture_generic_guard").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (parent, _, _) = capture_fixture(&store, "generic_guard").await;
    let request = begin(&parent);
    assert_generic_save_rejected(&store, &parent, request.sandbox.clone()).await;
    store
        .commit_sandbox_lifecycle(request.clone())
        .await
        .unwrap();
    let current = request.sandbox;
    let mut erased = current.clone();
    erased.lifecycle.checkpoint_capture = None;
    erased.revision += 1;
    assert_generic_save_rejected(&store, &current, erased).await;
    client.database(&name).drop().await.unwrap();
}

async fn assert_generic_save_rejected(store: &MongoSandboxStore, current: &Sandbox, next: Sandbox) {
    let events = store.sandbox_events(&current.id, 0, 10).await.unwrap();
    assert_eq!(
        store.save_sandbox(current.revision, next.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store
            .commit(SandboxTransaction::SaveSandboxAndAppend {
                command_revival: None,
                released_runtime: None,
                expected_revision: current.revision,
                sandbox: Box::new(next),
                event_at: NOW.into(),
                event: SandboxEventKind::WorkStepCompleted {
                    step: SandboxWorkStep::RunLifecycleOperation
                },
            })
            .await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&current.id).await.unwrap(),
        Some(current.clone())
    );
    assert_eq!(
        store.sandbox_events(&current.id, 0, 10).await.unwrap(),
        events
    );
}
