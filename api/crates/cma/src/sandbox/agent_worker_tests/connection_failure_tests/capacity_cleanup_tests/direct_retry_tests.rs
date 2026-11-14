//! Count actual complete-log Mongo finds through the real worker/store paths.
use super::*;
use crate::sandbox::agent_worker::WorkerFailure;
use layer2_core::sandbox::store::{
    plan_agent_cleanup_capacity_retry, plan_agent_marked_cleanup_capacity_retry, AgentWorkMutation,
    SandboxStoreError,
};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};

mod races;
mod transactions;
mod work_retry_policy_tests;

struct Fixture {
    client: Client,
    name: String,
    h: Harness<MongoSandboxStore>,
    scans: Arc<AtomicUsize>,
    app: String,
}
impl Fixture {
    async fn new() -> Self {
        let app = format!("cma-marked-retry-{}", uuid::Uuid::new_v4().simple());
        let scans = Arc::new(AtomicUsize::new(0));
        let observed = scans.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if event.command_name == "find"
                    && event.command.get_str("find").ok() == Some("agent_log")
                    && event
                        .command
                        .get_document("filter")
                        .is_ok_and(|filter| filter.len() == 1 && filter.contains_key("agent_id"))
                    && event.command.contains_key("sort")
                    && event.command.contains_key("limit")
                {
                    observed.fetch_add(1, Ordering::SeqCst);
                }
            }
        }));
        let (client, name, h) = fixture_with_client(Client::with_options(options).unwrap()).await;
        Self {
            client,
            name,
            h,
            scans,
            app,
        }
    }
    fn reset_scans(&self) {
        self.scans.store(0, Ordering::SeqCst);
    }
    fn scans(&self) -> usize {
        self.scans.load(Ordering::SeqCst)
    }
    async fn pending(&self, step: AgentWorkStep) -> Agent {
        let agent = self.h.admit("marked-retry", vec![]).await;
        for _ in 0..5 {
            self.h.run(&agent.id, SystemTime::now()).await;
        }
        let ready = self.h.stored(&agent.id).await;
        self.h
            .store
            .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
                target: AgentControlTarget::from(&ready),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(SystemTime::now()),
            })
            .await
            .unwrap();
        self.h.run(&agent.id, SystemTime::now()).await;
        if step != AgentWorkStep::CloseProcess {
            for _ in 0..2 {
                self.h.run(&agent.id, SystemTime::now()).await;
            }
            let stopped = self.h.stored(&agent.id).await;
            self.h
                .store
                .schedule_agent_delete(AgentDeleteRequest {
                    target: AgentControlTarget::from(&stopped),
                    requested_at: rfc3339_at(SystemTime::now()),
                })
                .await
                .unwrap();
            for _ in 0..match step {
                AgentWorkStep::RemoveHome => 1,
                AgentWorkStep::RemoveWorktree => 2,
                _ => 0,
            } {
                self.h.run(&agent.id, SystemTime::now()).await;
            }
        }
        let pending = fill_history(&self.client, &self.name, &self.h, &agent.id).await;
        assert_eq!(pending.work.step, step);
        pending
    }
    async fn claim(&self, id: &AgentId, at: SystemTime) -> Agent {
        self.h
            .store
            .claim_agent_work(
                id,
                "agent-worker",
                rfc3339_at(at),
                rfc3339_at(at + Duration::from_secs(18 * 60)),
            )
            .await
            .unwrap()
            .unwrap()
    }
    async fn fail(&self, claimed: &Agent, now: SystemTime) -> Result<(), SandboxStoreError> {
        let started = tokio::time::Instant::now();
        let result = self
            .h
            .worker
            .handle_failure(
                claimed.clone(),
                WorkerFailure::retryable("agent_cleanup_unavailable", "fixture operation failed"),
                now,
            )
            .await;
        self.h
            .worker
            .finish_capacity_attempt(claimed, result, now + started.elapsed())
            .await
    }
    async fn marked_claim(&self) -> (Agent, SystemTime) {
        let initial = self.pending(AgentWorkStep::CloseProcess).await;
        let now = SystemTime::now();
        let claimed = self.claim(&initial.id, now).await;
        self.fail(&claimed, now).await.unwrap();
        let saved = self.h.stored(&initial.id).await;
        let now = due(&saved);
        (self.claim(&initial.id, now).await, now)
    }
    async fn selected(&self, current: &Agent, now: SystemTime) -> AgentWorkMutation {
        let tail = self
            .h
            .store
            .agent_log_entry(&current.id, current.conversation.last_seq)
            .await
            .unwrap();
        plan_agent_marked_cleanup_capacity_retry(
            current,
            tail.as_ref(),
            "agent-worker",
            current.work.lease.as_ref().unwrap().fence,
            &rfc3339_at(now),
            &rfc3339_at(now + Duration::from_secs(30)),
        )
        .unwrap()
        .unwrap()
    }
    async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
fn due(agent: &Agent) -> SystemTime {
    SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(&agent.work.ready_at).unwrap())
}
fn assert_retry(current: &Agent, saved: &Agent, delay: u64) {
    assert_eq!(saved.work.step, current.work.step);
    assert_eq!(saved.status, current.status);
    assert_eq!(saved.work.attempts, current.work.attempts + 1);
    assert_eq!(saved.work.receipts, current.work.receipts);
    assert_eq!(saved.work.cleanup_debt, current.work.cleanup_debt);
    assert_eq!(saved.conversation, current.conversation);
    assert!(saved.work.lease.is_none());
    assert_eq!(
        layer2_core::seconds_from_rfc3339(&saved.work.ready_at).unwrap()
            - layer2_core::seconds_from_rfc3339(&saved.updated_at).unwrap(),
        delay
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn marked_failed_cleanup_uses_one_scan_versus_two_in_the_known_abort_control() {
    for step in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
    ] {
        let f = Fixture::new().await;
        let initial = f.pending(step).await;
        let now = SystemTime::now();
        let first = f.claim(&initial.id, now).await;
        f.reset_scans();
        f.fail(&first, now).await.unwrap();
        assert_eq!(f.scans(), 2, "first marker {step:?}");
        let saved = f.h.stored(&initial.id).await;
        assert_eq!(saved.conversation.last_seq, 10_001);
        let now = due(&saved);
        let current = f.claim(&initial.id, now).await;
        f.reset_scans();
        f.fail(&current, now).await.unwrap();
        assert_eq!(f.scans(), 1, "marked repeat {step:?}");
        let saved = f.h.stored(&initial.id).await;
        assert_retry(
            &current,
            &saved,
            if step == AgentWorkStep::RemoveWorktree {
                60
            } else {
                30
            },
        );
        // Reproduce the original normal-append/known-abort path on the same
        // marked state. Both actual transactions retain complete validation.
        let now = due(&saved);
        let current = f.claim(&initial.id, now).await;
        let mut ordinary = plan_agent_cleanup_capacity_retry(
            &current,
            None,
            "agent-worker",
            current.work.lease.as_ref().unwrap().fence,
            &rfc3339_at(now),
            &rfc3339_at(now + Duration::from_secs(30)),
        )
        .unwrap();
        ordinary.agent_logs = vec![AgentLogKind::WorkStepFailed {
            step,
            failure: layer2_core::sandbox::TypedFailure {
                class: "agent_cleanup_unavailable".into(),
                detail: "fixture operation failed".into(),
            },
        }];
        f.reset_scans();
        let result = f.h.store.commit_agent_work(ordinary).await;
        assert_eq!(result, Err(SandboxStoreError::AgentLogCapacityExceeded));
        f.h.worker
            .finish_capacity_attempt(&current, result, now)
            .await
            .unwrap();
        assert_eq!(f.scans(), 2, "known-abort control {step:?}");
        let saved = f.h.stored(&initial.id).await;
        assert_retry(
            &current,
            &saved,
            if step == AgentWorkStep::RemoveWorktree {
                120
            } else {
                30
            },
        );
        // Recovery still advances the original cleanup step and consumes only
        // its finite completion reserve. No retry fabricated a receipt.
        f.h.run(&initial.id, due(&saved)).await;
        let completed = f.h.stored(&initial.id).await;
        assert_ne!(completed.work.step, step);
        assert_eq!(completed.work.attempts, 0);
        assert_eq!(completed.conversation.last_seq, 10_002);
        f.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn real_failed_close_runs_the_selected_retry_without_repeating_diagnostics() {
    let f = Fixture::new().await;
    let initial = f.pending(AgentWorkStep::CloseProcess).await;
    let mut now = SystemTime::now();
    for attempt in 0..70 {
        f.h.backend.fail_next(
            WireOperation::CloseAgent,
            "runtime_unavailable",
            "fixture unavailable",
        );
        f.reset_scans();
        f.h.run(&initial.id, now).await;
        assert_eq!(f.scans(), if attempt == 0 { 2 } else { 1 });
        let saved = f.h.stored(&initial.id).await;
        assert_eq!(saved.work.attempts, attempt + 1);
        assert_eq!(saved.work.step, AgentWorkStep::CloseProcess);
        assert_eq!(saved.conversation.last_seq, 10_001);
        now = due(&saved);
    }
    f.h.run(&initial.id, now).await;
    assert_eq!(
        f.h.stored(&initial.id).await.work.step,
        AgentWorkStep::PublishStopped
    );
    f.cleanup().await;
}
