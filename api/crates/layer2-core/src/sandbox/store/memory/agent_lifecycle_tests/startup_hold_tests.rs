use super::*;
use crate::sandbox::store::{plan_agent_runtime_startup_hold_retry, SandboxStoreError};
use crate::sandbox::{AgentLogEntry, TypedFailure};

#[tokio::test]
async fn startup_hold_at_normal_history_limit_keeps_one_marker_and_pending_work() {
    let store = ready_store(1).await;
    let creation = created(store.admit_agent(admission("startup-hold")).await.unwrap());
    {
        let mut state = store.lock();
        let logs = state.agent_logs.get_mut(&creation.agent.id).unwrap();
        for seq in logs.len() as u64 + 1..=10_000 {
            logs.push(AgentLogEntry {
                agent_id: creation.agent.id.clone(),
                seq,
                appended_at: NOW.into(),
                kind: AgentLogKind::WorkStepFailed {
                    step: creation.agent.work.step,
                    failure: TypedFailure {
                        class: "ordinary_failure".into(),
                        detail: "temporary".into(),
                    },
                },
            });
        }
        state
            .agents
            .get_mut(&creation.agent.id)
            .unwrap()
            .conversation
            .last_seq = 10_000;
    }
    let original = store.lock().agent_logs[&creation.agent.id].clone();
    let mut at = NOW;
    for due in [MINUTE, TWO_MINUTES, "2026-09-05T00:03:00Z"] {
        let current = store
            .claim_agent_work(&creation.agent.id, "worker", at.into(), due.into())
            .await
            .unwrap()
            .unwrap();
        let tail = store
            .agent_log_entry(&current.id, current.conversation.last_seq)
            .await
            .unwrap();
        let mutation = plan_agent_runtime_startup_hold_retry(
            &current,
            tail.as_ref(),
            "worker",
            current.work.lease.as_ref().unwrap().fence,
            at,
            due,
        )
        .unwrap();
        assert_eq!(mutation.agent.conversation.last_seq, 10_001);
        let mut forged = mutation.clone();
        forged.fence += 1;
        assert_eq!(
            store.commit_agent_work(forged).await,
            Err(SandboxStoreError::Conflict)
        );
        store.commit_agent_work(mutation).await.unwrap();
        let held = store.get_agent(&current.id).await.unwrap().unwrap();
        assert_eq!(held.status, AgentStatus::Starting);
        assert_eq!(held.work.step, creation.agent.work.step);
        assert!(held.failure.is_none());
        let logs = &store.lock().agent_logs[&held.id];
        assert_eq!(logs.len(), 10_001);
        assert_eq!(&logs[..10_000], original.as_slice());
        at = due;
    }
}
