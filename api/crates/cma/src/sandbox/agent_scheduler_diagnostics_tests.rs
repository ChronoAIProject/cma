use super::super::lifecycle_failure::{CauseClass, DetailCode};
use super::*;
use std::io::Write;
use std::sync::Mutex;

#[path = "agent_scheduler_boundary_tests.rs"]
mod boundaries;

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
    fn events(&self) -> Vec<serde_json::Value> {
        self.text()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
    fn subscriber(&self, json: bool, filter: &str) -> Box<dyn tracing::Subscriber + Send + Sync> {
        let builder = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_writer(self.clone());
        if json {
            Box::new(builder.json().finish())
        } else {
            Box::new(builder.finish())
        }
    }
}

fn work() -> ScheduledWork {
    ScheduledWork::new(
        Arc::new(tests::Probe::default()),
        1,
        crate::background::BackgroundBudget::new(),
    )
}

#[tokio::test]
async fn terminal_failures_keep_metadata_and_drop_private_error_and_ancestor_fields() {
    for json in [false, true] {
        let output = Output::default();
        let _subscriber =
            tracing::subscriber::set_default(output.subscriber(json, crate::DEFAULT_LOG_FILTER));
        let caller = tracing::info_span!("private_caller", identity = "private-ancestor");
        let _entered = caller.enter();
        let mut work = work();
        let id = AgentId("private-agent".into());
        for (kind, origin, error, phase, cause) in [
            (
                TaskKind::Due,
                Origin::Discovery,
                SandboxStoreError::Unavailable("private-error".into()),
                "discovery",
                "store_unavailable",
            ),
            (
                TaskKind::Reconcile,
                Origin::Reconciliation,
                SandboxStoreError::Corrupt("private-error".into()),
                "reconciliation",
                "stored_data_invalid",
            ),
            (
                TaskKind::Agent(id.clone()),
                Origin::Claim,
                SandboxStoreError::Conflict,
                "execution",
                "store_conflict",
            ),
        ] {
            let metadata = TaskMetadata::new(kind);
            let expected = metadata.attempt.store(origin, error);
            let attempt_id = expected.observation.attempt_id.to_string();
            work.polling = phase == "discovery";
            work.reconciling = phase == "reconciliation";
            if phase == "execution" {
                work.in_flight.insert(id.clone());
            }
            let task = work.tasks.spawn(async move { Err(expected) });
            work.kinds.insert(task.id(), metadata);
            let result = work.tasks.join_next_with_id().await.unwrap();
            work.completed(result);
            assert!(!work.polling && !work.reconciling && work.in_flight.is_empty());
            assert!(work.kinds.is_empty());
            assert!(!output.text().contains("private-"));
            assert!(output.text().contains(&attempt_id));
            if json {
                let event = output.events().pop().unwrap();
                assert_eq!(event["fields"]["phase"], phase);
                assert_eq!(event["fields"]["cause_class"], cause);
                assert_eq!(event["span"], serde_json::json!({"name":"agent_scheduler"}));
                assert_eq!(
                    event["spans"],
                    serde_json::json!([{"name":"agent_scheduler"}])
                );
                assert!(serde_json::to_vec(&event["fields"]).unwrap().len() <= 2048);
            }
        }
        assert_eq!(output.text().lines().count(), 3);
    }
}

#[tokio::test]
async fn task_panic_and_cancellation_retain_admission_identity_and_free_capacity() {
    let output = Output::default();
    let _subscriber =
        tracing::subscriber::set_default(output.subscriber(true, crate::DEFAULT_LOG_FILTER));
    let caller = tracing::info_span!("private_caller", identity = "private-ancestor");
    let _entered = caller.enter();
    let mut work = work();
    for panic in [true, false] {
        let id = AgentId("private-agent".into());
        let metadata = TaskMetadata::new(TaskKind::Agent(id.clone()));
        let correlation = metadata.attempt.terminated(panic).attempt_id.to_string();
        let continuation = metadata.attempt.clone();
        let task = work.tasks.spawn(async move {
            continuation.continuation(4);
            if panic {
                panic!("private-panic-payload");
            }
            std::future::pending::<TaskResult>().await
        });
        work.kinds.insert(task.id(), metadata);
        work.in_flight.insert(id);
        tokio::task::yield_now().await;
        if !panic {
            task.abort();
        }
        let result = work.tasks.join_next_with_id().await.unwrap();
        work.completed(result);
        let event = output.events().pop().unwrap();
        assert_eq!(event["fields"]["attempt_id"], correlation);
        assert_eq!(event["fields"]["continuation_index"], 4);
        assert_eq!(
            event["fields"]["cause_class"],
            if panic {
                "task_panicked"
            } else {
                "task_cancelled"
            }
        );
        assert!(work.in_flight.is_empty() && work.kinds.is_empty());
    }
    assert_eq!(output.events().len(), 2);
    assert!(!output.text().contains("private-"));
}

#[test]
fn selectively_filtered_diagnostic_root_never_inherits_private_current_span() {
    for json in [false, true] {
        for global in [false, true] {
            let output = Output::default();
            // Reproduce a filter that accepts events but rejects the protective span.
            use tracing_subscriber::{layer::SubscriberExt, Layer};
            let filter = tracing_subscriber::filter::filter_fn(|metadata| {
                metadata.name() != "agent_scheduler"
            });
            let layer = tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(output.clone());
            let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = match (json, global) {
                (true, true) => Box::new(
                    tracing_subscriber::registry()
                        .with(layer.json())
                        .with(filter),
                ),
                (false, true) => Box::new(tracing_subscriber::registry().with(layer).with(filter)),
                (true, false) => {
                    Box::new(tracing_subscriber::registry().with(layer.json().with_filter(filter)))
                }
                (false, false) => {
                    Box::new(tracing_subscriber::registry().with(layer.with_filter(filter)))
                }
            };
            let _subscriber = tracing::subscriber::set_default(subscriber);
            let caller = tracing::info_span!("caller", identity = "private-ancestor");
            let _entered = caller.enter();
            let attempt = LifecycleAttempt::new(Phase::Execution, None);
            diagnostics::failed(&attempt.terminated(true));
            assert!(
                !output.text().contains("private-ancestor"),
                "{}",
                output.text()
            );
            if global {
                assert!(output.text().is_empty());
            }
        }
    }
}

#[test]
fn emitted_safe_detail_is_static_even_for_large_multiline_error() {
    let output = Output::default();
    let _subscriber =
        tracing::subscriber::set_default(output.subscriber(true, crate::DEFAULT_LOG_FILTER));
    let attempt = LifecycleAttempt::new(Phase::Execution, None);
    let value = attempt.ownership(
        Origin::Handoff,
        layer2_core::agent_ownership::OwnershipError::Unavailable(
            "receiving runtime revocation failed".into(),
        ),
    );
    assert_eq!(
        value.observation.detail_code,
        DetailCode::ReceiverRevocationUnconfirmed
    );
    diagnostics::failed(&value.observation);
    let value = attempt.ownership(
        Origin::OwnershipAcquire,
        layer2_core::agent_ownership::OwnershipError::Invalid("private-url\n秘密".repeat(100_000)),
    );
    assert_eq!(value.observation.cause_class, CauseClass::OwnershipInvalid);
    diagnostics::failed(&value.observation);
    assert_eq!(output.events().len(), 2);
    assert!(!output.text().contains("private-url"));
    assert!(!output.text().contains("秘密"));
}

#[test]
fn emitted_private_payloads_are_suppressed_and_largest_fields_stay_bounded() {
    use layer2_core::agent_ownership::OwnershipError;
    for json in [false, true] {
        let output = Output::default();
        let _subscriber =
            tracing::subscriber::set_default(output.subscriber(json, crate::DEFAULT_LOG_FILTER));
        let caller = tracing::info_span!("caller", identity = "private-ancestor");
        let _entered = caller.enter();
        let attempt = LifecycleAttempt::new(
            Phase::Reconciliation,
            Some(&AgentId("private-agent".into())),
        );
        for detail in [
            "https://user:private-sentinel@example.invalid/?token=private-sentinel".to_string(),
            "private-sentinel\n秘密\r\n{payload: private-sentinel}".into(),
            "private-sentinel".repeat(100_000),
        ] {
            for mut failure in [
                attempt.store(
                    Origin::Reconciliation,
                    SandboxStoreError::Unavailable(detail.clone()),
                ),
                attempt.store(
                    Origin::Reconciliation,
                    SandboxStoreError::Corrupt(detail.clone()),
                ),
                attempt.ownership(Origin::OwnershipAcquire, OwnershipError::Invalid(detail)),
            ] {
                failure.observation.elapsed_ms = u64::MAX;
                failure.observation.continuation_index = 6;
                diagnostics::failed(&failure.observation);
            }
        }
        assert!(!output.text().contains("private-"));
        assert!(!output.text().contains("秘密"));
        assert_eq!(output.text().lines().count(), 9);
        if json {
            for event in output.events() {
                assert!(serde_json::to_vec(&event["fields"]).unwrap().len() <= 2048);
                assert_eq!(event["fields"]["detail_suppressed"], true);
                assert_eq!(
                    event["spans"],
                    serde_json::json!([{"name":"agent_scheduler"}])
                );
            }
        }
    }
}
