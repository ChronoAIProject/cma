use super::*;
use crate::resource_access::{ResourceAccessStore, ResourceTarget};
use crate::sandbox::agent_io::*;
use crate::sandbox::store::{
    AgentWorkMutation, AgentWorkerStore, VersionedWorktree, WorktreeStore,
};
use crate::sandbox::{AgentLogKind, AgentWorkStep};

async fn pending() -> (InMemorySandboxStore, AgentAdmission) {
    let store = ready_store(2).await;
    let mut state = store.lock();
    let workspace = state.workspaces.get_mut(&workspace().id).unwrap();
    workspace.status = WorkspaceStatus::Preparing;
    workspace.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    workspace.resolved_commit = None;
    workspace.ready_at = None;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    let reference = crate::profiles::WorkspaceProfileRef::new(
        workspace.profile.id.clone(),
        workspace.profile.revision,
    );
    let parent = state.sandboxes.get_mut(&sandbox(2).id).unwrap();
    parent.origin = SandboxOrigin::Lifecycle {
        operation_id: "bootstrap-one".into(),
    };
    parent.status = SandboxStatus::Provisioning;
    parent.work = SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into());
    parent.profile.content.workspace_profiles = vec![reference];
    drop(state);
    let mut request = admission("unused");
    request.origin = AgentOrigin::Lifecycle {
        operation_id: "bootstrap-one".into(),
    };
    request.creation_key = format!(
        "lifecycle:{}",
        crate::profiles::content_digest(&request.origin).unwrap()
    );
    (store, request)
}

#[tokio::test]
async fn bootstrap_admission_reserves_one_real_agent_and_unresolved_worktree_before_runtime() {
    let (store, request) = pending().await;
    let (left, right) = tokio::join!(
        store.admit_agent(request.clone()),
        store.admit_agent(request.clone())
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_eq!(left.creation(), right.creation());
    assert!(
        matches!(left, AgentAdmissionResult::Created(_))
            != matches!(right, AgentAdmissionResult::Created(_))
    );
    let creation = left.creation();
    assert_eq!(creation.agent.work.step, AgentWorkStep::WaitForWorkspace);
    assert!(creation.worktree.base_commit.is_empty());
    assert!(creation.agent.conversation.conversation_id.is_none());
    let parent = store
        .get_sandbox(&creation.agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert!(parent.runtime.is_none());
    assert_eq!(parent.status, SandboxStatus::Provisioning);
    let workspace = store
        .get_workspace(&creation.agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        creation.agent.bootstrap_lineage(&parent, &workspace),
        Some("bootstrap-one")
    );
    assert_eq!(store.lock().worktrees.len(), 1);
    assert_eq!(workspace.next_worktree_number, 2);
    // Existing durable identity is replayable after the parent progresses.
    store.lock().sandboxes.get_mut(&parent.id).unwrap().status = SandboxStatus::Active;
    assert_eq!(
        store
            .admit_agent(request)
            .await
            .unwrap()
            .creation()
            .agent
            .id,
        creation.agent.id
    );
}

#[tokio::test]
async fn bootstrap_admission_rejects_unrelated_origin_owner_and_profile_before_any_allocation() {
    for variant in 0..5 {
        let (store, mut request) = pending().await;
        match variant {
            0 => {
                request.origin = AgentOrigin::Manual;
            }
            1 => {
                request.origin = AgentOrigin::Lifecycle {
                    operation_id: "another-operation".into(),
                };
            }
            2 => {
                request.owner = SubjectId::new("other");
            }
            3 => {
                store
                    .lock()
                    .workspaces
                    .get_mut(&workspace().id)
                    .unwrap()
                    .origin = WorkspaceOrigin::Manual;
            }
            _ => {
                store
                    .lock()
                    .sandboxes
                    .get_mut(&sandbox(2).id)
                    .unwrap()
                    .profile
                    .content
                    .workspace_profiles
                    .clear();
            }
        }
        assert!(
            store.admit_agent(request).await.is_err(),
            "variant {variant}"
        );
        assert!(store.lock().agents.is_empty());
        assert!(store.lock().worktrees.is_empty());
    }
}

fn binding_mutation(
    current: &crate::sandbox::Agent,
    worktree: &crate::sandbox::Worktree,
    commit: &str,
) -> AgentWorkMutation {
    let mut next = current.clone();
    let fence = current.work.lease.as_ref().unwrap().fence;
    next.work
        .advance("worker", fence, AgentWorkStep::AddWorktree, NOW.into())
        .unwrap();
    next.revision += 1;
    next.conversation.last_seq += 1;
    let mut bound = worktree.clone();
    bound.revision += 1;
    bound.base_commit = commit.into();
    AgentWorkMutation {
        expected_agent_revision: current.revision,
        holder: "worker".into(),
        fence,
        agent: next,
        worktree: Some(VersionedWorktree {
            expected_revision: worktree.revision,
            worktree: bound,
        }),
        appended_at: NOW.into(),
        sandbox_event: None,
        agent_logs: vec![AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::WaitForWorkspace,
        }],
    }
}

#[tokio::test]
async fn bootstrap_base_binding_requires_live_ready_parent_exact_commit_and_one_fenced_publication()
{
    let (store, request) = pending().await;
    let creation = created(store.admit_agent(request).await.unwrap());
    let current = store
        .claim_agent_work(
            &creation.agent.id,
            "worker",
            NOW.into(),
            "2026-09-05T00:01:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    let accepted = binding_mutation(&current, &creation.worktree, &"a".repeat(40));
    assert_eq!(
        store.commit_agent_work(accepted.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    {
        let mut state = store.lock();
        let parent = state.sandboxes.get_mut(&current.sandbox_id).unwrap();
        parent.status = SandboxStatus::Active;
        parent.credential.delivered = true;
        let workspace = state.workspaces.get_mut(&current.workspace_id).unwrap();
        workspace.status = WorkspaceStatus::Ready;
        workspace.resolved_commit = Some("b".repeat(40));
    }
    assert_eq!(
        store.commit_agent_work(accepted.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    store
        .lock()
        .workspaces
        .get_mut(&current.workspace_id)
        .unwrap()
        .resolved_commit = Some("a".repeat(40));
    let mut forged = accepted.clone();
    forged.worktree.as_mut().unwrap().worktree.owner = SubjectId::new("other");
    assert_eq!(
        store.commit_agent_work(forged).await,
        Err(SandboxStoreError::Conflict)
    );
    store.commit_agent_work(accepted.clone()).await.unwrap();
    assert_eq!(
        store.commit_agent_work(accepted).await,
        Err(SandboxStoreError::Conflict)
    );
    let agent = store.get_agent(&current.id).await.unwrap().unwrap();
    let worktree = store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent.work.step, AgentWorkStep::AddWorktree);
    assert_eq!(worktree.base_commit, "a".repeat(40));
    assert!(agent.work.receipts.is_empty());
    assert!(agent.work.cleanup_debt.is_empty());
}

async fn reservation(
    store: &InMemorySandboxStore,
    agent: &crate::sandbox::Agent,
    key: &str,
) -> CommandReservation {
    let target = ResourceTarget::Agent {
        agent_id: agent.id.clone(),
    };
    let snapshot = store.resource_access_snapshot(&target).await.unwrap();
    let command = AgentCommand::message("queued while bootstrap prepares");
    CommandReservation {
        binding: None,
        envelope: AgentCommandEnvelope {
            continuation: None,
            schema_version: AGENT_IO_SCHEMA_VERSION,
            command_id: crate::ids::AgentCommandId::generate(),
            agent_id: agent.id.clone(),
            owner: snapshot.scope(&target).unwrap().owner,
            actor: crate::resource_access::ResourceActor::Human {
                subject: agent.owner.clone(),
            },
            source: AgentCommandSource {
                kind: AgentSourceKind::Api,
                reference: "bootstrap-test".into(),
            },
            correlation_id: "bootstrap-test".into(),
            request_fingerprint: command.fingerprint().unwrap(),
            authorization: AgentAuthorizationRevision {
                authority_id: "owner-custody".into(),
                policy_revision: 1,
                grant_revision: None,
            },
            command,
        },
        key_sha256: command_key_digest(key).unwrap(),
        response_id: Some(crate::ids::AgentResponseId::generate()),
        admitted_at: NOW.into(),
        queue_deadline: "2026-09-05T00:01:00Z".into(),
        queue_capacity: 8,
        expected_agent_revision: agent.revision,
        parent: crate::sandbox::store::SandboxMutationGuard::capture(&snapshot.lineage.sandbox),
        policy: snapshot.fence(),
        authority_deadline: None,
        admission_audit: None,
    }
}

#[tokio::test]
async fn bootstrap_v2_queue_accepts_exact_parent_once_and_refuses_changed_workspace_or_stop() {
    let (store, input) = pending().await;
    let creation = created(store.admit_agent(input).await.unwrap());
    let request = reservation(&store, &creation.agent, "early-message").await;
    let accepted = store.reserve_command(request.clone()).await.unwrap();
    let receipt = match &accepted {
        CommandReservationResult::Created(record) => record,
        _ => panic!("expected new command"),
    };
    assert_eq!(receipt.status, AgentResponseStatus::Queued);
    assert!(receipt.work.dispatch.is_none());
    assert!(matches!(
        store.reserve_command(request.clone()).await.unwrap(),
        CommandReservationResult::Existing(_)
    ));
    assert!(store
        .get_agent(&creation.agent.id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .active_turn
        .is_none());

    store
        .lock()
        .workspaces
        .get_mut(&creation.agent.workspace_id)
        .unwrap()
        .status = WorkspaceStatus::Failed;
    let changed = reservation(&store, &creation.agent, "failed-workspace").await;
    assert_eq!(
        store.reserve_command(changed).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    store
        .lock()
        .workspaces
        .get_mut(&creation.agent.workspace_id)
        .unwrap()
        .status = WorkspaceStatus::Preparing;
    let mut stopping = creation.agent.clone();
    stopping.requested_stop_reason = Some(crate::sandbox::AgentStopReason::Owner);
    store
        .lock()
        .agents
        .insert(stopping.id.clone(), stopping.clone());
    let stop = reservation(&store, &stopping, "stopped-message").await;
    assert_eq!(
        store.reserve_command(stop).await.unwrap_err(),
        CommandLedgerError::NotAdmissible
    );
    // An accepted key keeps its real original receipt even after admission closes.
    assert!(matches!(
        store.reserve_command(request).await.unwrap(),
        CommandReservationResult::Existing(_)
    ));
}

#[tokio::test]
async fn bootstrap_ready_workspace_still_waits_for_parent_activation_without_binding_early() {
    let (store, request) = pending().await;
    {
        let mut state = store.lock();
        let workspace = state.workspaces.get_mut(&workspace().id).unwrap();
        workspace.status = WorkspaceStatus::Ready;
        workspace.resolved_commit = Some("a".repeat(40));
        state.sandboxes.get_mut(&sandbox(2).id).unwrap().status = SandboxStatus::Installing;
    }
    let creation = created(store.admit_agent(request).await.unwrap());
    assert_eq!(creation.agent.work.step, AgentWorkStep::WaitForWorkspace);
    assert!(creation.worktree.base_commit.is_empty());
    let parent = store
        .get_sandbox(&creation.agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let workspace = store
        .get_workspace(&creation.agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        creation.agent.bootstrap_lineage(&parent, &workspace),
        Some("bootstrap-one")
    );
}

#[tokio::test]
async fn bootstrap_workspace_claim_waits_without_mutation_until_installation_can_prepare_it() {
    use crate::sandbox::store::WorkspaceWorkerStore;
    let (store, _) = pending().await;
    let id = workspace().id;
    let original = store.get_workspace(&id).await.unwrap().unwrap();
    for status in [SandboxStatus::Provisioning, SandboxStatus::Installing] {
        store
            .lock()
            .sandboxes
            .get_mut(&sandbox(2).id)
            .unwrap()
            .status = status;
        assert!(store
            .claim_workspace_work(
                &id,
                "workspace-worker",
                NOW.into(),
                "2026-09-05T00:01:00Z".into()
            )
            .await
            .unwrap()
            .is_none());
        assert_eq!(store.get_workspace(&id).await.unwrap().unwrap(), original);
    }
    {
        let mut state = store.lock();
        let parent = state.sandboxes.get_mut(&sandbox(2).id).unwrap();
        parent.credential.delivered = true;
        parent.runtime = Some(crate::sandbox::RuntimeBinding {
            boot_id: None,
            provision_generation: Some(1),
            runtime_id: "actual-runtime".into(),
            runtime_version: "test".into(),
            bound_at: NOW.into(),
        });
    }
    let claimed = store
        .claim_workspace_work(
            &id,
            "workspace-worker",
            NOW.into(),
            "2026-09-05T00:01:00Z".into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.work.step, WorkspaceWorkStep::Prepare);
    assert_eq!(claimed.work.attempts, 0);
    assert_eq!(claimed.revision, original.revision + 1);
    assert!(claimed.failed.is_none());
}

#[tokio::test]
async fn trigger_task_reusing_legacy_lifecycle_agent_keeps_task_purpose() {
    use crate::sandbox::{FirstMessagePurpose, FirstMessageRecord};
    let store = ready_store(2).await;
    let mut initial = admission("lifecycle:legacy-operation");
    initial.origin = AgentOrigin::Lifecycle {
        operation_id: "legacy-operation".into(),
    };
    initial.creation_key = format!(
        "lifecycle:{}",
        crate::profiles::content_digest(&initial.origin).unwrap()
    );
    let mut agent = created(store.admit_agent(initial).await.unwrap()).agent;
    let original_origin = agent.origin.clone();
    let mut task = admission("trigger-task");
    task.origin = AgentOrigin::Trigger {
        trigger_id: "trigger".into(),
        launch_id: "launch".into(),
    };
    task.first_message = Some(FirstMessageRecord::new("trigger-task", "Actual task", NOW).unwrap());
    assert!(task.can_reuse(&agent, &workspace(), true));
    assert!(task
        .initialize_reused_agent(&mut agent, false)
        .unwrap()
        .is_some());
    assert_eq!(agent.origin, original_origin);
    assert_eq!(
        agent.conversation.first_message.as_ref().unwrap().purpose,
        Some(FirstMessagePurpose::Task)
    );
    assert_eq!(
        agent.conversation.first_message.as_ref().unwrap().text,
        "Actual task"
    );
}
