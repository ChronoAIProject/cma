use super::*;
use std::io::Write;
use std::sync::Mutex;

struct NoWork;

#[async_trait::async_trait]
impl AgentScheduleWork for NoWork {
    async fn due(&self, _now: SystemTime, _limit: u16) -> Result<Vec<AgentId>, SandboxStoreError> {
        Ok(Vec::new())
    }

    async fn reconcile(&self, _now: SystemTime) -> Result<usize, SandboxStoreError> {
        Ok(0)
    }

    async fn run_one(&self, _id: &AgentId, _now: SystemTime) -> Result<bool, SandboxStoreError> {
        Ok(false)
    }
}

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
    fn events(&self) -> Vec<serde_json::Value> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync {
        tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_writer(self.clone())
            .finish()
    }
}

#[tokio::test]
async fn scheduler_errors_keep_stage_and_hash_without_private_payload() {
    let output = Output::default();
    let _subscriber = tracing::subscriber::set_default(output.subscriber());
    let private = "private-agent-fixture";
    let caller = tracing::info_span!("caller", private_identity = private);
    let _entered = caller.enter();
    let probe = Arc::new(NoWork);
    let mut work = ScheduledWork::new(probe, 1, crate::background::BackgroundBudget::new());
    let id = AgentId(private.into());
    let failures = [
        (
            TaskKind::Due,
            "discovery",
            SandboxStoreError::Unavailable(private.into()),
            "store_unavailable",
        ),
        (
            TaskKind::Reconcile,
            "reconciliation",
            SandboxStoreError::Corrupt(private.into()),
            "stored_data_invalid",
        ),
        (
            TaskKind::Agent(id.clone()),
            "execution",
            SandboxStoreError::Conflict,
            "conflict_unclassified",
        ),
        (
            TaskKind::Agent(id.clone()),
            "execution",
            SandboxStoreError::NotFound,
            "domain_refusal",
        ),
    ];
    for (kind, phase, error, failure) in failures {
        work.polling = matches!(kind, TaskKind::Due);
        work.reconciling = matches!(kind, TaskKind::Reconcile);
        let is_agent = matches!(&kind, TaskKind::Agent(_));
        if is_agent {
            work.in_flight.insert(id.clone());
        }
        let task = work.tasks.spawn(async move { Err(error) });
        let task_id = task.id();
        work.kinds.insert(task_id, kind);
        let result = work.tasks.join_next_with_id().await.unwrap();
        work.completed(result);
        assert!(!work.polling && !work.reconciling && work.in_flight.is_empty());
        assert!(work.kinds.is_empty());
        let events = output.events();
        let event = events.last().unwrap();
        assert_eq!(event["target"], "agent_scheduler_failure");
        assert_eq!(event["fields"]["phase"], phase);
        assert_eq!(event["fields"]["failure"], failure);
        if is_agent {
            assert_eq!(
                event["fields"]["agent_hash"],
                format!(
                    "{:x}",
                    <sha2::Sha256 as sha2::Digest>::digest(id.as_str().as_bytes())
                )
            );
        } else {
            assert!(event["fields"].get("agent_hash").is_none());
        }
        assert!(!event.to_string().contains(private));
        assert_eq!(
            event["span"],
            serde_json::json!({"name": "agent_scheduler"})
        );
        assert_eq!(
            event["spans"],
            serde_json::json!([{"name": "agent_scheduler"}])
        );
    }
    assert_eq!(output.events().len(), 4);
}

#[tokio::test]
async fn scheduler_panic_and_cancellation_release_slots_without_logging_payload() {
    let output = Output::default();
    let _subscriber = tracing::subscriber::set_default(output.subscriber());
    let caller = tracing::info_span!("caller", private_identity = "private-caller-fixture");
    let _entered = caller.enter();
    let probe = Arc::new(NoWork);
    let mut work = ScheduledWork::new(probe, 1, crate::background::BackgroundBudget::new());
    let id = AgentId("private-agent-fixture".into());
    for panic in [true, false] {
        let task = work.tasks.spawn(async move {
            if panic {
                panic!("private-panic-fixture");
            }
            std::future::pending::<TaskResult>().await
        });
        work.kinds.insert(task.id(), TaskKind::Agent(id.clone()));
        work.in_flight.insert(id.clone());
        if !panic {
            task.abort();
        }
        let result = work.tasks.join_next_with_id().await.unwrap();
        assert!(result.is_err());
        work.completed(result);
        assert!(work.in_flight.is_empty() && work.kinds.is_empty());
    }
    let events = output.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["fields"]["failure"], "task_panicked");
    assert_eq!(events[1]["fields"]["failure"], "task_cancelled");
    assert!(!serde_json::to_string(&events).unwrap().contains("private-"));
    for event in events {
        assert_eq!(
            event["span"],
            serde_json::json!({"name": "agent_scheduler"})
        );
        assert_eq!(
            event["spans"],
            serde_json::json!([{"name": "agent_scheduler"}])
        );
    }
}

#[tokio::test]
async fn successful_discovery_and_completion_emit_no_failure() {
    let output = Output::default();
    let _subscriber = tracing::subscriber::set_default(output.subscriber());
    let probe = Arc::new(NoWork);
    let mut work = ScheduledWork::new(probe, 1, crate::background::BackgroundBudget::new());
    let id = AgentId("private-agent-fixture".into());
    let discovered = id.clone();
    let task = work
        .tasks
        .spawn(async { Ok(TaskOutput::Due(vec![discovered])) });
    work.kinds.insert(task.id(), TaskKind::Due);
    let result = work.tasks.join_next_with_id().await.unwrap();
    work.completed(result);
    assert_eq!(work.queued.pop_front(), Some(id.clone()));
    work.in_flight.insert(id.clone());
    let task = work.tasks.spawn(async { Ok(TaskOutput::Completed(1)) });
    work.kinds.insert(task.id(), TaskKind::Agent(id));
    let result = work.tasks.join_next_with_id().await.unwrap();
    work.completed(result);
    assert!(work.in_flight.is_empty() && work.kinds.is_empty());
    assert!(output.events().is_empty());
}
