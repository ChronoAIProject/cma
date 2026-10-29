use super::agent_admission_tests::{admission, ready_store, NOW};
use super::*;
use crate::sandbox::store::{
    AgentAdmissionStore, AgentControlStore, AgentControlStoreError, AgentCreation,
};
use crate::sandbox::{
    AgentCleanupDebt, AgentStatus, AgentWorkStep, FirstMessageRecord, TypedFailure, WorktreeStatus,
};
use crate::{Authorization, Caller, Capability};
use layer3_adapter::WorktreeRequest;

fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

async fn failed_store(limit: u8) -> (InMemorySandboxStore, Agent) {
    let store = ready_store(limit).await;
    let mut request = admission("manual-recovery");
    request.first_message =
        Some(FirstMessageRecord::new("manual-recovery", "Keep this task", NOW).unwrap());
    let created = store.admit_agent(request).await.unwrap();
    let mut agent = created.creation().agent.clone();
    let mut worktree = created.creation().worktree.clone();
    let work = WorktreeRequest::new(
        "worktree:project:wt-1:add:v1",
        "project",
        "wt-1",
        worktree.base_commit.clone(),
    );
    agent.work.cleanup_debt.push(AgentCleanupDebt {
        step: AgentWorkStep::AddWorktree,
        operation_id: work.operation_id,
        request_sha256: work.request_sha256,
    });
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    agent
        .fail(
            TypedFailure {
                class: "agent_runtime_rejected".into(),
                detail: "runtime_mutation_outcome_unknown: partly created worktree after checkout deadline".into(),
            },
            NOW.into(),
        )
        .unwrap();
    worktree.transition(WorktreeStatus::Failed).unwrap();
    {
        let mut state = store.lock();
        state
            .sandboxes
            .get_mut(&agent.sandbox_id)
            .unwrap()
            .credential
            .delivered = true;
        state.worktrees.insert(worktree.id.clone(), worktree);
        state.agents.insert(agent.id.clone(), agent.clone());
    }
    (store, agent)
}

async fn recover(
    store: &InMemorySandboxStore,
    agent: &Agent,
    key: &str,
) -> Result<Agent, AgentControlStoreError> {
    store
        .begin_agent_worktree_recovery(
            &agent.id,
            &owner(),
            key,
            &WorktreeId::generate(),
            NOW.into(),
        )
        .await
}

#[tokio::test]
async fn recovery_keeps_agent_history_provenance_and_failed_worktree_while_allocating_once() {
    let (store, original) = failed_store(2).await;
    let before = store.lock().clone();
    let (left, right) = tokio::join!(
        recover(&store, &original, "same"),
        recover(&store, &original, "same")
    );
    let next = left.unwrap();
    assert_eq!(next, right.unwrap());
    assert_eq!(next.id, original.id);
    assert_eq!(next.conversation, original.conversation);
    assert_eq!(next.profile, original.profile);
    assert_eq!(next.status, AgentStatus::Starting);
    assert_eq!(next.work.step, AgentWorkStep::AddWorktree);
    assert!(next.work.cleanup_debt.is_empty());
    assert_eq!(
        next.work.worktree_recoveries[0].original_cleanup_debt,
        original.work.cleanup_debt[0]
    );
    assert_eq!(
        next.work.worktree_recoveries[0].original_failure,
        original.failure.unwrap()
    );
    let state = store.lock();
    let old_tree = &state.worktrees[&original.worktree_id];
    assert_eq!(old_tree.path, before.worktrees[&original.worktree_id].path);
    assert_eq!(
        old_tree.branch,
        before.worktrees[&original.worktree_id].branch
    );
    assert_eq!(
        old_tree.base_commit,
        before.worktrees[&original.worktree_id].base_commit
    );
    assert_eq!(old_tree.status, WorktreeStatus::Failed);
    assert_eq!(old_tree.agent_id, None);
    assert_eq!(state.worktrees.len(), 2);
    assert_eq!(
        state.workspaces[&original.workspace_id].next_worktree_number,
        3
    );
    assert_eq!(state.worktrees[&next.worktree_id].name, "wt-2");
    assert_eq!(
        state.worktrees[&next.worktree_id].base_commit,
        old_tree.base_commit
    );
    assert_eq!(state.agent_logs, before.agent_logs);
    let creation = AgentCreation {
        agent: next.clone(),
        worktree: state.worktrees[&next.worktree_id].clone(),
        initial_log: state.agent_logs[&next.id][0].clone(),
    };
    creation.validate_replay().unwrap();
    crate::sandbox::agent_reads_fold::fold_agent_log(&next, &state.agent_logs[&next.id]).unwrap();
    let parent = &state.sandboxes[&next.sandbox_id];
    let provenance = crate::sandbox::AgentCreationProvenance::from_legacy(
        &next,
        parent,
        crate::resource_access::ResourceOwner::Person {
            subject: next.owner.clone(),
        },
        Some(&creation.initial_log),
    )
    .unwrap();
    assert_eq!(provenance.worktree_id, original.worktree_id);
}

#[tokio::test]
async fn recovery_authorization_operator_audit_and_deleted_replay_fail_closed() {
    let (store, original) = failed_store(2).await;
    for caller in [
        Caller::new(SubjectId::new("foreign"), owner().authorization),
        Caller::new(SubjectId::new("owner"), Authorization::new([])),
    ] {
        assert_eq!(
            store
                .begin_agent_worktree_recovery(
                    &original.id,
                    &caller,
                    "attempt",
                    &WorktreeId::generate(),
                    NOW.into()
                )
                .await
                .unwrap_err(),
            AgentControlStoreError::NotFound
        );
    }
    let admin = Caller::new(
        SubjectId::new("operator"),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    let mut next = store
        .begin_agent_worktree_recovery(
            &original.id,
            &admin,
            "attempt",
            &WorktreeId::generate(),
            NOW.into(),
        )
        .await
        .unwrap();
    assert_eq!(next.work.worktree_recoveries[0].requested_by, admin.subject);
    assert_eq!(
        recover(&store, &next, "attempt").await.unwrap_err(),
        AgentControlStoreError::WorktreeRecoveryConflict
    );
    next.status = AgentStatus::Deleted;
    next.deleted_at = Some(NOW.into());
    next.work.schedule(AgentWorkStep::None, NOW.into());
    next.revision += 1;
    store.lock().agents.insert(next.id.clone(), next.clone());
    assert_eq!(
        store
            .begin_agent_worktree_recovery(
                &next.id,
                &admin,
                "attempt",
                &WorktreeId::generate(),
                NOW.into()
            )
            .await
            .unwrap(),
        next
    );
    assert_eq!(
        recover(&store, &next, "new").await.unwrap_err(),
        AgentControlStoreError::WorktreeRecoveryConflict
    );
    assert_eq!(store.lock().worktrees.len(), 2);
}

#[tokio::test]
async fn recovery_rejects_any_started_effect_or_altered_failure_without_mutation() {
    for case in 0..14 {
        let (store, mut original) = failed_store(2).await;
        match case {
            0 => original.ready_at = Some(NOW.into()),
            1 => original.conversation.conversation_id = Some("used".into()),
            2 => original.conversation.runtime_generation = Some(1),
            3 => original.work.warm_conversation_id = Some("used".into()),
            4 => original.work.resolved_skills = Some(vec![]),
            5 => original.work.cleanup_debt[0].step = AgentWorkStep::MaterializeHome,
            6 => original.work.cleanup_debt[0].request_sha256 = "wrong".into(),
            7 => original.work.cleanup_debt[0].operation_id = "different-operation".into(),
            8 => original.failure.as_mut().unwrap().class = "unknown".into(),
            9 => original.work.step = AgentWorkStep::RemoveWorktree,
            10 => {
                original
                    .conversation
                    .first_message
                    .as_mut()
                    .unwrap()
                    .attempt = 1
            }
            11 => original.status = AgentStatus::Stopped,
            12 => original.deleted_at = Some(NOW.into()),
            13 => original.work.next_fence = u64::MAX,
            _ => unreachable!(),
        }
        store
            .lock()
            .agents
            .insert(original.id.clone(), original.clone());
        assert!(
            recover(&store, &original, "attempt").await.is_err(),
            "case {case}"
        );
        let state = store.lock();
        assert_eq!(state.agents[&original.id], original);
        assert_eq!(state.worktrees.len(), 1);
        assert_eq!(
            state.workspaces[&original.workspace_id].next_worktree_number,
            2
        );
    }
}

#[tokio::test]
async fn recovery_requires_idle_parents_preserves_quota_and_rejects_overlapping_attempts() {
    for case in 0..5 {
        let (store, original) = failed_store(1).await;
        {
            let mut state = store.lock();
            match case {
                0 => {
                    state
                        .sandboxes
                        .get_mut(&original.sandbox_id)
                        .unwrap()
                        .status = SandboxStatus::Rotating
                }
                1 => {
                    state
                        .sandboxes
                        .get_mut(&original.sandbox_id)
                        .unwrap()
                        .credential
                        .delivered = false
                }
                2 => {
                    state
                        .workspaces
                        .get_mut(&original.workspace_id)
                        .unwrap()
                        .status = crate::sandbox::WorkspaceStatus::Failed
                }
                3 => {
                    state
                        .workspaces
                        .get_mut(&original.workspace_id)
                        .unwrap()
                        .next_worktree_number = 1
                }
                4 => {
                    let mut live = original.clone();
                    live.id = AgentId::generate();
                    live.status = AgentStatus::Ready;
                    state.agents.insert(live.id.clone(), live);
                }
                _ => unreachable!(),
            }
        }
        assert!(
            recover(&store, &original, "attempt").await.is_err(),
            "case {case}"
        );
        assert_eq!(store.lock().worktrees.len(), 1);
    }
    let (store, original) = failed_store(2).await;
    let (left, right) = tokio::join!(
        recover(&store, &original, "left"),
        recover(&store, &original, "right")
    );
    assert_eq!(
        [left, right].iter().filter(|result| result.is_ok()).count(),
        1
    );
    assert_eq!(store.lock().worktrees.len(), 2);
}

#[tokio::test]
async fn recovery_chain_retains_all_failures_and_old_replay_never_allocates_again() {
    let (store, original) = failed_store(2).await;
    let mut next = recover(&store, &original, "first").await.unwrap();
    {
        let mut state = store.lock();
        let tree = state.worktrees.get_mut(&next.worktree_id).unwrap();
        tree.transition(WorktreeStatus::Failed).unwrap();
        let request = WorktreeRequest::new(
            "worktree:project:wt-2:add:v1",
            "project",
            "wt-2",
            tree.base_commit.clone(),
        );
        next.work.cleanup_debt = vec![AgentCleanupDebt {
            step: AgentWorkStep::AddWorktree,
            operation_id: request.operation_id,
            request_sha256: request.request_sha256,
        }];
        next.work.schedule(AgentWorkStep::None, NOW.into());
        next.fail(
            TypedFailure {
                class: "agent_runtime_rejected".into(),
                detail: "runtime_mutation_outcome_unknown: second incomplete checkout".into(),
            },
            NOW.into(),
        )
        .unwrap();
        state.agents.insert(next.id.clone(), next.clone());
    }
    assert_eq!(recover(&store, &next, "first").await.unwrap(), next);
    let latest = recover(&store, &next, "second").await.unwrap();
    assert_eq!(latest.work.worktree_recoveries.len(), 2);
    assert_eq!(latest.creation_worktree_id(), &original.worktree_id);
    assert_eq!(recover(&store, &latest, "first").await.unwrap(), latest);
    let mut corrupt = latest.clone();
    corrupt.work.worktree_recoveries[0].successor_worktree_id = WorktreeId::generate();
    assert!(corrupt.validate_worktree_recoveries().is_err());
    assert_eq!(store.lock().worktrees.len(), 3);
}

#[tokio::test]
async fn ordinary_agent_save_cannot_rebind_or_modify_recovery_archive() {
    use crate::sandbox::store::AgentStore;
    let (store, original) = failed_store(2).await;
    let recovered = recover(&store, &original, "first").await.unwrap();
    for case in 0..3 {
        let mut changed = recovered.clone();
        changed.revision += 1;
        match case {
            0 => changed.worktree_id = original.worktree_id.clone(),
            1 => changed.work.worktree_recoveries.clear(),
            2 => changed.work.worktree_recoveries[0].original_failure.detail = "rewritten".into(),
            _ => unreachable!(),
        }
        assert_eq!(
            store
                .save_agent(recovered.revision, changed)
                .await
                .unwrap_err(),
            SandboxStoreError::Conflict
        );
    }
    assert_eq!(
        store.get_agent(&recovered.id).await.unwrap().unwrap(),
        recovered
    );
}
