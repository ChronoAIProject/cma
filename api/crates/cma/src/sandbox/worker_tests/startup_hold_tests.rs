use super::*;
use layer2_core::sandbox::work_retry::{work_retry_policy, WorkRetryFailure};
use layer3_adapter::{AdapterError, RuntimeStartupFailure, RuntimeStartupFailureClass};

#[tokio::test]
async fn startup_hold_backoff_applies_with_retry_policy_on_or_off_and_preserves_larger_bases() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let mut sandbox = h.create().await;
    for enabled in [false, true] {
        for base in [1, 30, 60, 180, 600] {
            let mut config =
                SandboxWorkerConfig::new("startup-hold", "test", runtime_image(), h.policy.clone())
                    .with_work_retry_policy(enabled);
            config.retry_delay = Duration::from_secs(base);
            let worker = SandboxWorker::new(
                SandboxWorkerDependencies {
                    store: h.store.clone(),
                    adapter: h.adapter.clone(),
                    credentials: h.credentials.clone(),
                    workspaces: Arc::new(WorkspaceRecorder {
                        actions: h.actions.clone(),
                    }),
                    agents: Arc::new(AgentStopRecorder {
                        actions: h.actions.clone(),
                    }),
                    registry: h.registry.clone(),
                    updates: DurableAgentUpdateRecorder::new(h.store.clone()),
                    operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
                },
                config,
            );
            for (attempt, seconds) in [30, 60, 120, 240, 300, 300].into_iter().enumerate() {
                sandbox.work.attempts = attempt as u32;
                let expected = Duration::from_secs(seconds.max(base));
                let expected = if enabled {
                    expected.max(
                        work_retry_policy(
                            crate::sandbox::work_retry::sandbox_class(&sandbox),
                            WorkRetryFailure::Transient,
                            sandbox.work.attempts,
                            Duration::from_secs(base),
                        )
                        .delay,
                    )
                } else {
                    expected
                };
                let error = AdapterError::RuntimeStartupHold(RuntimeStartupFailure {
                    class: RuntimeStartupFailureClass::ExitedBeforeServing,
                    consecutive: 3,
                });
                assert_eq!(
                    worker.native_retry_delay_for_test(&sandbox, error),
                    expected
                );
                let ordinary = if enabled {
                    work_retry_policy(
                        crate::sandbox::work_retry::sandbox_class(&sandbox),
                        WorkRetryFailure::Transient,
                        sandbox.work.attempts,
                        Duration::from_secs(base),
                    )
                    .delay
                } else {
                    Duration::from_secs(base)
                };
                assert_eq!(
                    worker.native_retry_delay_for_test(
                        &sandbox,
                        AdapterError::Unavailable("temporary".into()),
                    ),
                    ordinary,
                    "ordinary retries retain their policy"
                );
            }
        }
    }
}
