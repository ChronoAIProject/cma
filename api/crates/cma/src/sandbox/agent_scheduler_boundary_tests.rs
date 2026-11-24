use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct BoundaryWork {
    deadline: bool,
    fail: AtomicBool,
    admitted: Mutex<Vec<(AgentId, uuid::Uuid)>>,
    release: Notify,
}

#[async_trait::async_trait]
impl AgentScheduleWork for BoundaryWork {
    async fn due(&self, _: SystemTime, _: u16) -> Result<Vec<AgentId>, SandboxStoreError> {
        if self.deadline {
            std::future::pending::<()>().await;
        }
        Err(SandboxStoreError::Unavailable("private-discovery".into()))
    }

    async fn reconcile(&self, _: SystemTime) -> Result<usize, SandboxStoreError> {
        Err(SandboxStoreError::Corrupt("private-reconciliation".into()))
    }

    async fn reconcile_slice(
        &self,
        _: SystemTime,
        _: &mut ConnectionSweep,
    ) -> Result<bool, SandboxStoreError> {
        Err(SandboxStoreError::Corrupt("private-reconciliation".into()))
    }

    async fn run_one(
        &self,
        id: &AgentId,
        _: SystemTime,
        attempt: &LifecycleAttempt,
    ) -> Result<bool, LifecycleTaskFailure> {
        self.admitted
            .lock()
            .unwrap()
            .push((id.clone(), attempt.terminated(false).attempt_id));
        if id.as_str() == "private-a" {
            self.release.notified().await;
        } else {
            self.release.notify_one();
        }
        if self.fail.load(Ordering::Relaxed) {
            attempt.continuation(if id.as_str() == "private-a" { 6 } else { 2 });
            return Err(attempt.store(Origin::ContinuationRead, SandboxStoreError::Conflict));
        }
        Ok(id.as_str() == "private-a")
    }
}

#[tokio::test(start_paused = true)]
async fn discovery_and_reconciliation_errors_are_projected_at_real_spawn_boundaries() {
    for deadline in [false, true] {
        let output = Output::default();
        let _subscriber = tracing::subscriber::set_default(
            output.subscriber(true, "agent_scheduler_failure=error"),
        );
        let worker = Arc::new(BoundaryWork {
            deadline,
            ..Default::default()
        });
        let mut work = ScheduledWork::new(worker, 2, crate::background::BackgroundBudget::new());
        work.poll(true);
        let expected: Vec<_> = work
            .kinds
            .values()
            .map(|task| {
                (
                    task.attempt.phase(),
                    task.attempt.terminated(false).attempt_id.to_string(),
                )
            })
            .collect();
        while let Some(result) = work.tasks.join_next_with_id().await {
            work.completed(result);
        }
        let events = output.events();
        assert_eq!(events.len(), 2);
        for (phase, id) in expected {
            let fields = &events
                .iter()
                .find(|event| event["fields"]["phase"] == phase)
                .unwrap()["fields"];
            assert_eq!(fields["origin"], phase);
            assert_eq!(fields["attempt_id"], id);
            assert!(fields.get("agent_hash").is_none());
            assert_eq!(
                fields["detail_code"],
                if deadline && phase == "discovery" {
                    "discovery_deadline"
                } else {
                    "unstructured_detail_suppressed"
                }
            );
        }
        assert!(!output.text().contains("private-"));
        assert!(!work.polling && !work.reconciling && work.kinds.is_empty());
    }
}

#[tokio::test]
async fn interleaved_agents_keep_context_and_success_or_claim_loss_emit_no_failure() {
    use sha2::{Digest, Sha256};
    let output = Output::default();
    let _subscriber =
        tracing::subscriber::set_default(output.subscriber(true, crate::DEFAULT_LOG_FILTER));
    let worker = Arc::new(BoundaryWork {
        fail: AtomicBool::new(true),
        ..Default::default()
    });
    let mut work = ScheduledWork::new(
        worker.clone(),
        2,
        crate::background::BackgroundBudget::new(),
    );
    let ids = [AgentId("private-a".into()), AgentId("private-b".into())];
    work.queued.extend(ids.clone());
    work.fill_slots();
    while let Some(result) = work.tasks.join_next_with_id().await {
        work.completed(result);
    }
    assert_eq!(output.events().len(), 2);
    for (id, attempt) in worker.admitted.lock().unwrap().iter() {
        let events = output.events();
        let fields = &events
            .iter()
            .find(|event| event["fields"]["attempt_id"] == attempt.to_string())
            .unwrap()["fields"];
        assert_eq!(
            fields["agent_hash"],
            format!("{:x}", Sha256::digest(id.as_str().as_bytes()))
        );
        assert_eq!(
            fields["continuation_index"],
            if id == &ids[0] { 6 } else { 2 }
        );
        assert_eq!(fields["origin"], "continuation_read");
    }
    {
        let admitted = worker.admitted.lock().unwrap();
        assert_ne!(admitted[0].1, admitted[1].1);
    }
    worker.fail.store(false, Ordering::Relaxed);
    work.queued.extend(ids);
    work.fill_slots();
    while let Some(result) = work.tasks.join_next_with_id().await {
        work.completed(result);
    }
    assert_eq!(
        output.events().len(),
        2,
        "recovery and claim loss must not emit stale diagnostics"
    );
    assert!(work.in_flight.is_empty() && work.kinds.is_empty());
}

#[tokio::test]
async fn missing_task_metadata_has_only_a_safe_uncorrelated_fallback() {
    let output = Output::default();
    let _subscriber =
        tracing::subscriber::set_default(output.subscriber(true, crate::DEFAULT_LOG_FILTER));
    let mut work = work();
    let task = work.tasks.spawn(std::future::pending::<TaskResult>());
    task.abort();
    let result = work.tasks.join_next_with_id().await.unwrap();
    work.completed(result);
    let events = output.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["fields"]["origin"], "unspecified");
    assert_eq!(events[0]["fields"]["cause_class"], "task_cancelled");
    assert!(events[0]["fields"].get("agent_hash").is_none());
}
