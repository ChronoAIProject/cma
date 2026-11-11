//! Exercises the production orchestration, with effects recorded at its ports.
use super::*;
use crate::store::agent_capacity_recovery::records::test_support;
use layer2_core::sandbox::{agent_capacity_recovery::state::ConfirmedNativeState, AgentWorkLease};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Default)]
enum Outcome {
    #[default]
    Recorded,
    Unknown,
    Abort,
    WrongReceipt,
    Refused,
}
#[derive(Default)]
struct Fixture {
    events: Mutex<Vec<&'static str>>,
    completion: Outcome,
    reconciliation: Outcome,
    terminal: bool,
    completed: bool,
    claim_unknown: bool,
    deny_activation: bool,
    unavailable: bool,
    timeout: bool,
    wrong_confirmation: bool,
}
impl Fixture {
    fn event(&self, event: &'static str) {
        self.events.lock().unwrap().push(event);
    }
    fn assert_events(&self, expected: &[&str]) {
        assert_eq!(*self.events.lock().unwrap(), expected);
    }
}
fn finished(claim: &RecoveryClaim, now: &str) -> RecoveryWork {
    let mut work = claim.work.clone();
    work.phase = RecoveryPhase::Completed;
    work.revision += 1;
    work.completed_at = Some(now.into());
    work.lease = None;
    work
}
fn result(
    outcome: Outcome,
    work: RecoveryWork,
) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
    Ok(match outcome {
        Outcome::Recorded => CommitOutcome::Recorded(work),
        Outcome::Unknown => CommitOutcome::Unknown,
        Outcome::Abort => CommitOutcome::KnownAbort,
        Outcome::Refused => return Err(RecoveryError::Conflict),
        Outcome::WrongReceipt => {
            let mut wrong = work;
            wrong.next_fence += 1;
            CommitOutcome::Recorded(wrong)
        }
    })
}
#[async_trait::async_trait]
impl AcceptedWork for Arc<Fixture> {
    async fn claim(
        &self,
        _: &RecoveryRequest,
        holder: &str,
        _: &str,
        expires: &str,
    ) -> Result<ClaimOutcome, RecoveryError> {
        self.event("claim");
        if self.claim_unknown {
            return Ok(ClaimOutcome::Unknown);
        }
        let mut work = test_support::work();
        if self.completed {
            work.phase = RecoveryPhase::Completed;
            work.revision = 3;
            work.attempts = 1;
            work.completed_at = Some(test_support::NOW.into());
            return Ok(ClaimOutcome::Terminal(Box::new(work)));
        }
        if self.terminal {
            work.phase = RecoveryPhase::Superseded;
            return Ok(ClaimOutcome::Terminal(Box::new(work)));
        }
        work.phase = RecoveryPhase::Attaching;
        work.revision += 1;
        work.attempts += 1;
        work.lease = Some(AgentWorkLease {
            holder: holder.into(),
            fence: work.next_fence,
            expires_at: expires.into(),
        });
        work.next_fence += 1;
        Ok(ClaimOutcome::Claimed(Box::new(RecoveryClaim {
            work,
            agent: test_support::agent(),
        })))
    }
    async fn complete(
        &self,
        claim: &RecoveryClaim,
        _: &RecoveryConfirmation,
        now: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.event("complete");
        result(self.completion, finished(claim, now))
    }
    async fn retry(
        &self,
        claim: &RecoveryClaim,
        now: &str,
        retry_at: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.event("retry");
        assert!(retry_at > now);
        let lease = claim.work.lease.as_ref().unwrap();
        Ok(CommitOutcome::Recorded(claim.work.retry(
            &lease.holder,
            lease.fence,
            now,
            retry_at,
        )?))
    }
    async fn reconcile(
        &self,
        expected: &RecoveryWork,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        self.event("reconcile");
        result(self.reconciliation, expected.clone())
    }
    async fn require_activation(&self, _: &RecoveryWork) -> Result<(), RecoveryError> {
        self.event("authorize_activation");
        if self.deny_activation {
            Err(RecoveryError::Unauthorized)
        } else {
            Ok(())
        }
    }
}
struct Attachment(Arc<Fixture>);
impl Drop for Attachment {
    fn drop(&mut self) {
        self.0.event("drop_attachment");
    }
}
struct Runtime(Arc<Fixture>);
impl std::ops::Deref for Runtime {
    type Target = Fixture;
    fn deref(&self) -> &Fixture {
        &self.0
    }
}
#[async_trait::async_trait]
impl CapacityRecoveryRuntime for Runtime {
    type Attachment = Attachment;
    async fn reattach_existing(
        &self,
        claim: &RecoveryClaim,
    ) -> Result<(Attachment, RecoveryConfirmation), RecoveryError> {
        self.event("restricted_attach");
        if self.timeout {
            std::future::pending::<()>().await;
        }
        if self.unavailable {
            return Err(RecoveryError::Unavailable);
        }
        let confirmation = RecoveryConfirmation {
            operation_key: claim.work.request.key()?,
            target: claim.work.target.clone(),
            authority_fence: claim.work.lease.as_ref().unwrap().fence
                + u64::from(self.wrong_confirmation),
            native_state: ConfirmedNativeState::Idle,
        };
        Ok((Attachment(self.0.clone()), confirmation))
    }
    async fn acknowledge(&self, _: Attachment, _: &RecoveryWork) -> Result<(), RecoveryError> {
        self.event("activate");
        Ok(())
    }
}
async fn execute(fixture: &Arc<Fixture>) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
    let at = SystemTime::UNIX_EPOCH
        + Duration::from_secs(layer2_core::seconds_from_rfc3339(test_support::NOW).unwrap());
    run(
        fixture,
        &Runtime(fixture.clone()),
        &test_support::work().request,
        "worker",
        at,
    )
    .await
}

#[tokio::test(start_paused = true)]
async fn exact_completion_and_fresh_authority_precede_activation() {
    let fixture = Arc::new(Fixture::default());
    assert!(
        matches!(execute(&fixture).await.unwrap(), CommitOutcome::Recorded(work) if work.phase == RecoveryPhase::Completed)
    );
    fixture.assert_events(&[
        "claim",
        "restricted_attach",
        "complete",
        "authorize_activation",
        "activate",
        "drop_attachment",
    ]);
}

#[tokio::test(start_paused = true)]
async fn uncertain_completion_reconciles_without_repeating_mutation_or_attachment() {
    for reconciliation in [Outcome::Recorded, Outcome::Unknown, Outcome::WrongReceipt] {
        let fixture = Arc::new(Fixture {
            completion: Outcome::Unknown,
            reconciliation,
            ..Fixture::default()
        });
        let outcome = execute(&fixture).await;
        let mut expected = vec!["claim", "restricted_attach", "complete", "reconcile"];
        match reconciliation {
            Outcome::Recorded => {
                assert!(matches!(outcome.unwrap(), CommitOutcome::Recorded(_)));
                expected.extend(["authorize_activation", "activate"]);
            }
            Outcome::Unknown => assert_eq!(outcome.unwrap(), CommitOutcome::Unknown),
            _ => assert_eq!(outcome, Err(RecoveryError::InvalidEvidence)),
        }
        expected.push("drop_attachment");
        fixture.assert_events(&expected);
    }
}

#[tokio::test(start_paused = true)]
async fn aborted_refused_or_mismatched_completion_never_activates() {
    for completion in [Outcome::Abort, Outcome::Refused, Outcome::WrongReceipt] {
        let fixture = Arc::new(Fixture {
            completion,
            ..Fixture::default()
        });
        let outcome = execute(&fixture).await;
        match completion {
            Outcome::Abort => assert_eq!(outcome.unwrap(), CommitOutcome::KnownAbort),
            Outcome::Refused => assert_eq!(outcome, Err(RecoveryError::Conflict)),
            _ => assert_eq!(outcome, Err(RecoveryError::InvalidEvidence)),
        }
        fixture.assert_events(&["claim", "restricted_attach", "complete", "drop_attachment"]);
    }
}

#[tokio::test(start_paused = true)]
async fn changed_authority_or_unconfirmed_native_state_drops_without_activation() {
    for wrong_confirmation in [false, true] {
        let fixture = Arc::new(Fixture {
            wrong_confirmation,
            deny_activation: true,
            ..Fixture::default()
        });
        assert!(execute(&fixture).await.is_err());
        let mut expected = vec!["claim", "restricted_attach"];
        if !wrong_confirmation {
            expected.extend(["complete", "authorize_activation"]);
        }
        expected.push("drop_attachment");
        fixture.assert_events(&expected);
    }
}

#[tokio::test(start_paused = true)]
async fn unavailable_and_timeout_are_paced_without_input_or_activation() {
    for timeout in [false, true] {
        let fixture = Arc::new(Fixture {
            unavailable: true,
            timeout,
            ..Fixture::default()
        });
        assert!(
            matches!(execute(&fixture).await.unwrap(), CommitOutcome::Recorded(work) if work.phase == RecoveryPhase::Admitted)
        );
        fixture.assert_events(&["claim", "restricted_attach", "retry"]);
    }
}

#[tokio::test(start_paused = true)]
async fn uncertain_claim_and_superseded_work_make_no_runtime_call() {
    for terminal in [false, true] {
        let fixture = Arc::new(Fixture {
            terminal,
            claim_unknown: !terminal,
            ..Fixture::default()
        });
        assert!(execute(&fixture).await.is_ok());
        fixture.assert_events(&["claim"]);
    }
}

#[tokio::test(start_paused = true)]
async fn completed_work_without_activation_proof_cannot_report_activated_success() {
    let fixture = Arc::new(Fixture {
        completed: true,
        ..Fixture::default()
    });
    assert_eq!(execute(&fixture).await, Err(RecoveryError::Unavailable));
    fixture.assert_events(&["claim"]);
}
