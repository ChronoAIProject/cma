use super::*;
use crate::ids::{AgentCommandId, AgentResponseId};
use crate::sandbox::store::Page;
use std::sync::Mutex;
use std::time::SystemTime;

// The worker receives an already released ledger record. Any attempted claim,
// preparation, or dispatch while readiness is unavailable fails this fixture.
struct QueuedStore(Mutex<AdmittedAgentCommand>);

#[async_trait::async_trait]
impl AgentCommandStore for QueuedStore {
    async fn reserve_command(
        &self,
        _: CommandReservation,
    ) -> Result<CommandReservationResult, CommandLedgerError> {
        panic!("retry must not readmit the message")
    }
    async fn get_command(
        &self,
        id: &AgentCommandId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        let record = self.0.lock().unwrap();
        assert_eq!(id, &record.receipt.envelope.command_id);
        Ok(Some(record.clone()))
    }
    async fn get_response_command(
        &self,
        _: &AgentResponseId,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        panic!("unexpected response lookup")
    }
}

#[async_trait::async_trait]
impl AgentCommandWorkerStore for QueuedStore {
    async fn command_work_page(
        &self,
        _: &PageRequest,
    ) -> Result<Page<AdmittedAgentCommand>, CommandLedgerError> {
        panic!("unexpected discovery")
    }
    async fn reject_unclaimed_command(
        &self,
        id: &AgentCommandId,
        revision: u64,
        code: CommandFailureCode,
        now: String,
    ) -> Result<(), CommandLedgerError> {
        let mut record = self.0.lock().unwrap();
        assert_eq!(id, &record.receipt.envelope.command_id);
        record.reject_unclaimed(revision, code, now)
    }
    async fn reconcile_command(
        &self,
        _: &AgentCommandId,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        panic!("no dispatch exists to reconcile")
    }
    async fn claim_next_command(
        &self,
        _: CommandClaimRequest,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        panic!("unavailable readiness must not claim")
    }
    async fn claim_observed_command(
        &self,
        _: CommandClaimRequest,
        _: &AgentCommandId,
        _: u64,
    ) -> Result<Option<AdmittedAgentCommand>, CommandLedgerError> {
        panic!("unavailable readiness must not claim")
    }
    async fn renew_command_claim(
        &self,
        _: &CommandClaim,
        _: String,
        _: String,
    ) -> Result<(), CommandLedgerError> {
        panic!("no execution lease may be acquired")
    }
    async fn release_command_claim(
        &self,
        _: &CommandClaim,
        _: String,
    ) -> Result<(), CommandLedgerError> {
        panic!("no execution lease may be acquired")
    }
    async fn reject_command(
        &self,
        _: &CommandClaim,
        _: CommandFailureCode,
        _: String,
    ) -> Result<AdmittedAgentCommand, CommandLedgerError> {
        panic!("no execution lease may be acquired")
    }
}

struct RetryThenRevoke {
    checks: AtomicUsize,
    entered: Notify,
}

#[async_trait::async_trait]
impl AgentCommandExecution for RetryThenRevoke {
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        if self.checks.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.notify_one();
            CommandReadiness::Retry
        } else {
            CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked)
        }
    }
    async fn prepare_queued(&self, _: &AdmittedAgentCommand) {
        panic!("failed readiness must not prepare the target")
    }
    async fn execute(&self, _: &AdmittedAgentCommand) -> CommandAttempt {
        panic!("failed readiness must not dispatch input")
    }
}

async fn queued(age: u64, source: AgentSourceKind) -> AdmittedAgentCommand {
    let (_, agent, parent) = fixture(AgentStatus::Ready).await;
    let mut reservation = super::super::super::ledger_tests::reservation(&agent, &parent, "pacing");
    reservation.admitted_at = crate::rfc3339_at(SystemTime::now() - Duration::from_secs(age));
    reservation.queue_deadline = crate::rfc3339_at(SystemTime::now() + Duration::from_secs(300));
    reservation.envelope.source.kind = source;
    reservation.envelope.actor = match source {
        AgentSourceKind::Channel => AgentActor::Channel {
            bridge_id: "bridge".into(),
            grant_id: "grant".into(),
            platform: "telegram".into(),
            chat_id: "chat".into(),
            sender_id: "sender".into(),
        },
        AgentSourceKind::Trigger => AgentActor::Trigger {
            subject: agent.owner.clone(),
            launch_id: crate::LaunchId::generate(),
        },
        _ => AgentActor::Human {
            subject: agent.owner.clone(),
        },
    };
    let mut record = reservation.record(1, 0, None).unwrap();
    record.status = AgentResponseStatus::Queued;
    record.validate().unwrap();
    record
}

async fn assert_retry_preserves_receipt_then_rechecks_revocation(
    record: AdmittedAgentCommand,
    expected_delay: Duration,
) {
    let store = Arc::new(QueuedStore(Mutex::new(record.clone())));
    let execution = Arc::new(RetryThenRevoke {
        checks: AtomicUsize::new(0),
        entered: Notify::new(),
    });
    let worker = AgentCommandWorker::new(store.clone(), execution.clone(), "first".into()).unwrap();
    let id = record.receipt.envelope.command_id.clone();
    let run_id = id.clone();
    let task = tokio::spawn(async move { worker.run_one(&run_id).await });
    execution.entered.notified().await;
    tokio::task::yield_now().await;
    tokio::time::advance(expected_delay - Duration::from_millis(1)).await;
    assert!(!task.is_finished());
    assert_eq!(store.get_command(&id).await.unwrap(), Some(record.clone()));
    tokio::time::advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert!(task.is_finished());
    task.await.unwrap().unwrap();
    assert_eq!(store.get_command(&id).await.unwrap(), Some(record.clone()));

    AgentCommandWorker::new(store.clone(), execution.clone(), "restart".into())
        .unwrap()
        .run_one(&id)
        .await
        .unwrap();
    let rejected = store.get_command(&id).await.unwrap().unwrap();
    assert_eq!(rejected.receipt, record.receipt);
    assert_eq!(rejected.queue_deadline, record.queue_deadline);
    assert_eq!(
        rejected.work.failure.unwrap().code,
        CommandFailureCode::AuthorityRevoked
    );
    assert!(rejected.work.lease.is_none());
    assert!(rejected.work.dispatch.is_none());
    assert_eq!(execution.checks.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn fresh_channel_retry_is_prompt_and_older_outages_back_off_without_new_receipts() {
    for (age, seconds) in [(0, 1), (30, 5), (120, 15)] {
        let record = queued(age, AgentSourceKind::Channel).await;
        assert_retry_preserves_receipt_then_rechecks_revocation(
            record,
            Duration::from_secs(seconds),
        )
        .await;
    }
}

#[tokio::test(start_paused = true)]
async fn manual_trigger_and_incomplete_channel_attribution_keep_normal_retry_pacing() {
    for source in [AgentSourceKind::UserInterface, AgentSourceKind::Trigger] {
        let record = queued(0, source).await;
        assert_retry_preserves_receipt_then_rechecks_revocation(record, Duration::from_secs(15))
            .await;
    }
    let mut record = queued(0, AgentSourceKind::Channel).await;
    record.receipt.envelope.source.kind = AgentSourceKind::Api;
    assert_retry_preserves_receipt_then_rechecks_revocation(record, Duration::from_secs(15)).await;
}

#[tokio::test(start_paused = true)]
async fn channel_retry_never_extends_the_original_queue_deadline() {
    let mut record = queued(40, AgentSourceKind::Channel).await;
    record.queue_deadline = crate::rfc3339_at(SystemTime::now() + Duration::from_secs(2));
    assert_retry_preserves_receipt_then_rechecks_revocation(record, Duration::from_secs(2)).await;
}
