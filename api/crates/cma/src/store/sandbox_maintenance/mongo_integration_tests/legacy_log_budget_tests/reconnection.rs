use super::certification::automatic;
use super::*;
use layer2_core::sandbox::store::{
    AgentConnectionRecoveryCandidate, AgentConnectionRecoveryOutcome, AgentWorkMutation,
    AgentWorkPublicationLineage, AgentWorkPublicationParents, AgentWorkerStore, WorktreeStore,
};
use layer2_core::sandbox::{InactivityPauseRefusal, Worktree, WorktreeStatus};

const RECOVERY_AT: &str = "2026-09-05T00:27:00Z";
const REPEATED_AT: &str = "2026-09-05T00:28:00Z";

async fn recover(store: &MongoSandboxStore, agent: &Agent, at: &str) -> Agent {
    let candidate = AgentConnectionRecoveryCandidate::from_agent(agent).unwrap();
    assert_eq!(
        store
            .schedule_agent_connection_recovery(candidate.restart_request(at.into()))
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::Scheduled
    );
    let claimed = store
        .claim_agent_work(&agent.id, "reconnect", at.into(), LATER.into())
        .await
        .unwrap()
        .unwrap();
    let fence = claimed.work.lease.as_ref().unwrap().fence;
    let mut next = claimed.clone();
    next.work
        .advance("reconnect", fence, AgentWorkStep::None, at.into())
        .unwrap();
    next.updated_at = at.into();
    next.revision += 1;
    next.conversation.last_seq += 1;
    store
        .commit_agent_connection_work(
            AgentWorkMutation {
                expected_agent_revision: claimed.revision,
                holder: "reconnect".into(),
                fence,
                agent: next.clone(),
                worktree: None,
                appended_at: at.into(),
                sandbox_event: None,
                agent_logs: vec![AgentLogKind::WorkStepCompleted {
                    step: AgentWorkStep::PublishConnection,
                }],
            },
            AgentWorkPublicationParents {
                sandbox_revision: 1,
                workspace_revision: 1,
                worktree_revision: 1,
                lineage: AgentWorkPublicationLineage::Authorized,
            },
        )
        .await
        .unwrap();
    let entries = store
        .agent_log(&agent.id, agent.conversation.last_seq, 2)
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, next.conversation.last_seq);
    assert_eq!(entries[0].appended_at, at);
    assert_eq!(
        entries[0].kind,
        AgentLogKind::WorkStepCompleted {
            step: AgentWorkStep::PublishConnection
        }
    );
    next
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn quiet_connection_recovery_keeps_the_material_clock_and_exact_log_head() {
    let f = TestStore::open().await;
    for (id, rows, recent_work) in [
        ("sbx_quiet_recovery", 3, false),
        ("sbx_certified_recovery", 257, false),
        ("sbx_recent_work_recovery", 3, true),
    ] {
        let sandbox = active_sandbox(id);
        f.store.create_sandbox(sandbox.clone()).await.unwrap();
        let mut agent = ready_agent(&sandbox, &format!("agt_{id}"));
        agent.conversation.last_seq = rows;
        let mut history = small_history(&agent, rows);
        if recent_work {
            history.last_mut().unwrap().appended_at = RECOVERY_AT.into();
        }
        seed_legacy(&f.store, &agent, &history).await;
        let workspace = super::super::runtime_update_integration_tests::workspace(&sandbox);
        f.store.create_workspace(workspace).await.unwrap();
        f.store
            .create_worktree(Worktree {
                id: agent.worktree_id.clone(),
                workspace_id: agent.workspace_id.clone(),
                sandbox_id: agent.sandbox_id.clone(),
                owner: agent.owner.clone(),
                name: "wt-1".into(),
                path: "/home/cma/update/.worktrees/wt-1".into(),
                branch: "cma/wt-1".into(),
                base_commit: "a".repeat(40),
                status: WorktreeStatus::Ready,
                agent_id: Some(agent.id.clone()),
                revision: 1,
            })
            .await
            .unwrap();

        let recovered = recover(&f.store, &agent, RECOVERY_AT).await;
        let recovered = recover(&f.store, &recovered, REPEATED_AT).await;
        let request = f.request(&sandbox);
        if rows > MAX_LEGACY_ROWS as u64 {
            for _ in 0..3 {
                assert_eq!(
                    automatic(&f, &sandbox).await,
                    SandboxMaintenanceOutcome::NotEligible
                );
            }
        }
        let preview = f
            .store
            .preview_inactivity_pause_with_reason(request.clone())
            .await
            .unwrap();
        assert_eq!(
            preview.refusal,
            recent_work.then_some(InactivityPauseRefusal::RecentAgentActivity)
        );
        assert_unchanged(&f.store, &sandbox).await;
        assert_eq!(
            automatic(&f, &sandbox).await,
            if recent_work {
                SandboxMaintenanceOutcome::NotEligible
            } else {
                SandboxMaintenanceOutcome::Scheduled
            }
        );
        assert_eq!(f.store.get_agent(&agent.id).await.unwrap(), Some(recovered));
    }
    f.cleanup().await;
}
