use super::ledger_hold_tests::{due, prepare, preserved, worker, CASES};
use super::*;
use crate::sandbox::agent_worker::WorkerFailure;
use layer2_core::sandbox::store::{AGENT_RUNTIME_STARTUP_HOLD, AGENT_RUNTIME_STARTUP_HOLD_DETAIL};

mod sibling;

#[tokio::test]
async fn startup_hold_retries_keep_starting_and_cleanup_work_pending_across_worker_takeover() {
    for (step, _, _) in CASES {
        let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let initial = prepare(&h, step).await;
        let mut at = due(&initial).max(h.now);
        for (attempt, delay) in [30, 60, 120, 240, 300, 300].into_iter().enumerate() {
            let holder = format!("hold-worker-{attempt}");
            let worker = worker(&h, h.store.clone(), &holder);
            let claimed = h
                .store
                .claim_agent_work(
                    &initial.id,
                    &holder,
                    rfc3339_at(at),
                    rfc3339_at(at + Duration::from_secs(900)),
                )
                .await
                .unwrap()
                .unwrap();
            worker
                .handle_failure(
                    claimed,
                    WorkerFailure::retryable(
                        AGENT_RUNTIME_STARTUP_HOLD,
                        AGENT_RUNTIME_STARTUP_HOLD_DETAIL,
                    ),
                    at,
                )
                .await
                .unwrap();
            let held = h.stored(&initial.id).await;
            preserved(&initial, &held);
            assert_eq!(held.work.step, step);
            assert_eq!(held.status, initial.status);
            assert_eq!(held.work.attempts, attempt as u32 + 1);
            assert_eq!(
                held.conversation.last_seq,
                initial.conversation.last_seq + 1
            );
            assert_eq!(due(&held), at + Duration::from_secs(delay), "{step:?}");
            let marker = h
                .store
                .agent_log_entry(&held.id, held.conversation.last_seq)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(marker.kind, AgentLogKind::WorkStepFailed { failure, .. }
                if failure.class == AGENT_RUNTIME_STARTUP_HOLD)
            );
            at = due(&held);
        }
    }
}

#[tokio::test]
async fn startup_hold_keeps_larger_bases_and_optional_policy_delays() {
    use layer2_core::sandbox::work_retry::{work_retry_policy, WorkRetryFailure};

    for step in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::PublishConnection,
    ] {
        for enabled in [false, true] {
            for base in [1, 60, 600] {
                let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
                let initial = prepare(&h, step).await;
                let mut config = AgentWorkerConfig::new("startup-base", DEPLOYMENT_ID)
                    .with_work_retry_policy(enabled);
                config.retry_delay = Duration::from_secs(base);
                let worker = AgentWorker::new(
                    AgentWorkerDependencies {
                        store: h.store.clone(),
                        adapter: h.adapter.clone(),
                        skills: Arc::new(EmptyAgentSkillResolver),
                        registry: h.registry.clone(),
                        updates: h.updates.clone(),
                        operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
                    },
                    config,
                );
                let mut at = due(&initial).max(h.now);
                for attempt in 0..14 {
                    let claimed = h
                        .store
                        .claim_agent_work(
                            &initial.id,
                            "startup-base",
                            rfc3339_at(at),
                            rfc3339_at(at + Duration::from_secs(60)),
                        )
                        .await
                        .unwrap()
                        .unwrap();
                    let floor = Duration::from_secs(
                        (base.max(30) * (1_u64 << attempt.min(4))).min(base.max(300)),
                    );
                    let expected = if enabled {
                        floor.max(
                            work_retry_policy(
                                crate::sandbox::work_retry::agent_class(step),
                                WorkRetryFailure::Transient,
                                attempt,
                                Duration::from_secs(base),
                            )
                            .delay,
                        )
                    } else {
                        floor
                    };
                    worker
                        .handle_failure(
                            claimed,
                            WorkerFailure::retryable(
                                AGENT_RUNTIME_STARTUP_HOLD,
                                AGENT_RUNTIME_STARTUP_HOLD_DETAIL,
                            ),
                            at,
                        )
                        .await
                        .unwrap();
                    let held = h.stored(&initial.id).await;
                    assert_eq!(due(&held), at + expected);
                    assert_eq!(held.work.step, step);
                    assert_eq!(held.work.attempts, attempt + 1);
                    at = due(&held);
                }
            }
        }
    }
}
