//! Transactional model stub, not a replacement for S1/S4 or Mongo proofs.
use super::*;
use crate::sandbox::agent_capacity_recovery::{ports::*, state::*};
use crate::{Authorization, Caller, Capability};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Default)]
pub(super) enum Fault {
    #[default]
    None,
    BeforeCommit,
    AfterCommit,
    UnknownAbsent,
}

pub(super) struct State {
    pub fixture: Fixture,
    pub work: Option<RecoveryWork>,
    pub writer_gate: bool,
    pub recovery_gate: bool,
    pub fault: Fault,
    pub unresolved: bool,
    pub successors: usize,
    pub publications: usize,
    pub source_available: bool,
    pub logs: Vec<AgentLogEntry>,
}

#[derive(Clone)]
pub(super) struct Store(pub Arc<Mutex<State>>);

impl Store {
    pub fn new(fixture: Fixture) -> Self {
        let failure = failure(&fixture);
        Self(Arc::new(Mutex::new(State {
            fixture,
            work: None,
            writer_gate: true,
            recovery_gate: true,
            fault: Fault::None,
            unresolved: false,
            successors: 0,
            publications: 0,
            source_available: true,
            logs: vec![failure],
        })))
    }

    pub fn request(&self) -> RecoveryRequest {
        let s = self.0.lock().unwrap();
        RecoveryRequest {
            agent_id: s.fixture.agent.id.clone(),
            creation_key: s.fixture.agent.creation_key.clone(),
            creation_fingerprint: s.fixture.agent.creation_fingerprint.clone(),
            failure_seq: s.fixture.agent.conversation.last_seq,
            requested_at: NOW.into(),
        }
    }

    pub fn caller(&self) -> Caller {
        Caller::new(
            self.0.lock().unwrap().fixture.agent.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        )
    }
}

fn failure(f: &Fixture) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: f.agent.id.clone(),
        seq: f.agent.conversation.last_seq,
        appended_at: f.agent.updated_at.clone(),
        kind: AgentLogKind::WorkStepFailed {
            step: AgentWorkStep::PublishConnection,
            failure: f.agent.failure.clone().unwrap(),
        },
    }
}

fn authorize(s: &State, caller: &Caller) -> Result<(), RecoveryError> {
    if caller.subject != s.fixture.agent.owner
        || !caller.authorization.allows(Capability::UseOwnSandboxes)
    {
        return Err(RecoveryError::Unauthorized);
    }
    Ok(())
}

fn request_matches(s: &State, request: &RecoveryRequest) -> Result<(), RecoveryError> {
    if s.work.as_ref().is_some_and(|work| &work.request != request) {
        return Err(RecoveryError::Conflict);
    }
    Ok(())
}

/// Only the capture stub can create this exact-snapshot token.
pub(super) struct Proof {
    source: Agent,
    parent_revision: u64,
}

#[async_trait]
impl CapacityRecoveryStore for Store {
    type Proof = Proof;

    async fn lookup(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
    ) -> Result<RecoveryLookup, RecoveryError> {
        let s = self.0.lock().unwrap();
        authorize(&s, caller)?;
        request_matches(&s, request)?;
        if s.unresolved {
            return Ok(RecoveryLookup::Unknown);
        }
        Ok(s.work.clone().map_or(RecoveryLookup::NotStarted, |w| {
            RecoveryLookup::Recorded(Box::new(w))
        }))
    }

    async fn capture(
        &self,
        caller: &Caller,
        _: &RecoveryRequest,
    ) -> Result<RecoverySnapshot<Proof>, RecoveryError> {
        let s = self.0.lock().unwrap();
        authorize(&s, caller)?;
        if !s.source_available {
            return Err(RecoveryError::Unavailable);
        }
        let f = &s.fixture;
        Ok(RecoverySnapshot {
            proof: Proof {
                source: f.agent.clone(),
                parent_revision: f.sandbox.revision,
            },
            agent: f.agent.clone(),
            sandbox: f.sandbox.clone(),
            workspace: f.workspace.clone(),
            worktree: f.worktree.clone(),
            original_failure: s.logs[0].clone(),
        })
    }

    async fn admit(
        &self,
        caller: &Caller,
        snapshot: RecoverySnapshot<Proof>,
        work: RecoveryWork,
        next_agent: Agent,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let mut s = self.0.lock().unwrap();
        authorize(&s, caller)?;
        request_matches(&s, &work.request)?;
        if !s.writer_gate || !s.recovery_gate {
            return Err(RecoveryError::Disabled);
        }
        if snapshot.proof.source != s.fixture.agent
            || snapshot.agent != s.fixture.agent
            || snapshot.proof.parent_revision != s.fixture.sandbox.revision
            || snapshot.sandbox != s.fixture.sandbox
            || snapshot.workspace != s.fixture.workspace
            || snapshot.worktree != s.fixture.worktree
            || s.work.is_some()
        {
            return Err(RecoveryError::Conflict);
        }
        let plan = s
            .fixture
            .context()
            .plan(&s.logs[0], &work.request.requested_at)
            .map_err(RecoveryError::Disposition)?;
        if plan
            .fenced_agent(&s.fixture.context(), &work.request.requested_at)
            .map_err(RecoveryError::Disposition)?
            != next_agent
            || RecoveryWork::prepare(work.request.clone(), &plan, &s.fixture.context())? != work
        {
            return Err(RecoveryError::InvalidEvidence);
        }
        let fault = std::mem::take(&mut s.fault);
        match fault {
            Fault::BeforeCommit => return Ok(CommitOutcome::KnownAbort),
            Fault::UnknownAbsent => {
                s.unresolved = true;
                return Ok(CommitOutcome::Unknown);
            }
            _ => {}
        }
        s.fixture.agent = next_agent;
        s.work = Some(work.clone());
        s.successors += 1;
        if matches!(fault, Fault::AfterCommit) {
            s.unresolved = true;
            return Ok(CommitOutcome::Unknown);
        }
        Ok(CommitOutcome::Recorded(work))
    }

    async fn claim(
        &self,
        caller: &Caller,
        request: &RecoveryRequest,
        holder: &str,
        now: &str,
        expires_at: &str,
    ) -> Result<ClaimOutcome, RecoveryError> {
        let mut s = self.0.lock().unwrap();
        authorize(&s, caller)?;
        request_matches(&s, request)?;
        if s.unresolved {
            return Ok(ClaimOutcome::Unknown);
        }
        let mut work = s.work.clone().ok_or(RecoveryError::Conflict)?;
        if matches!(
            work.phase,
            RecoveryPhase::Completed | RecoveryPhase::Superseded
        ) {
            return Ok(ClaimOutcome::Terminal(Box::new(work)));
        }
        if matches!(
            s.fixture.classify(),
            CapacityRecoveryDisposition::CleanupOnly
                | CapacityRecoveryDisposition::TerminalPreserved
        ) {
            work.phase = RecoveryPhase::Superseded;
            work.lease = None;
            work.revision += 1;
            s.work = Some(work.clone());
            return Ok(ClaimOutcome::Terminal(Box::new(work)));
        }
        let Some(claim) = work.claim(&s.fixture.context(), holder, now, expires_at)? else {
            return Ok(ClaimOutcome::Waiting(Box::new(work)));
        };
        s.fixture.agent = claim.agent.clone();
        s.work = Some(claim.work.clone());
        Ok(ClaimOutcome::Claimed(Box::new(claim)))
    }

    async fn complete(
        &self,
        caller: &Caller,
        claim: &RecoveryClaim,
        confirmation: &RecoveryConfirmation,
        now: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let mut s = self.0.lock().unwrap();
        authorize(&s, caller)?;
        let work = s.work.as_ref().ok_or(RecoveryError::Conflict)?;
        let (next_work, next_agent, log) =
            work.complete(&s.fixture.context(), claim, confirmation, now)?;
        let fault = std::mem::take(&mut s.fault);
        if matches!(fault, Fault::BeforeCommit) {
            return Ok(CommitOutcome::KnownAbort);
        }
        if matches!(fault, Fault::UnknownAbsent) {
            s.unresolved = true;
            return Ok(CommitOutcome::Unknown);
        }
        s.logs.push(AgentLogEntry {
            agent_id: next_agent.id.clone(),
            seq: next_agent.conversation.last_seq,
            appended_at: now.into(),
            kind: log,
        });
        s.fixture.agent = next_agent;
        s.work = Some(next_work.clone());
        s.publications += 1;
        if matches!(fault, Fault::AfterCommit) {
            s.unresolved = true;
            return Ok(CommitOutcome::Unknown);
        }
        Ok(CommitOutcome::Recorded(next_work))
    }

    async fn retry(
        &self,
        caller: &Caller,
        claim: &RecoveryClaim,
        now: &str,
        retry_at: &str,
    ) -> Result<CommitOutcome<RecoveryWork>, RecoveryError> {
        let mut s = self.0.lock().unwrap();
        authorize(&s, caller)?;
        if s.work.as_ref() != Some(&claim.work) {
            return Err(RecoveryError::Conflict);
        }
        let lease = claim.work.lease.as_ref().unwrap();
        let work = claim
            .work
            .retry(&lease.holder, lease.fence, now, retry_at)?;
        s.work = Some(work.clone());
        Ok(CommitOutcome::Recorded(work))
    }
}

#[derive(Default)]
pub(super) struct RuntimeState {
    pub reconnects: Vec<RecoveryTarget>,
    pub acknowledgements: usize,
    pub unavailable: bool,
    pub mismatch: bool,
    pub delay: std::time::Duration,
}

#[derive(Clone, Default)]
pub(super) struct Runtime(pub Arc<Mutex<RuntimeState>>);

#[async_trait]
impl CapacityRecoveryRuntime for Runtime {
    type Attachment = (String, u64);
    async fn reattach_existing(
        &self,
        claim: &RecoveryClaim,
    ) -> Result<(Self::Attachment, RecoveryConfirmation), RecoveryError> {
        let delay = self.0.lock().unwrap().delay;
        tokio::time::sleep(delay).await;
        let mut s = self.0.lock().unwrap();
        s.reconnects.push(claim.work.target.clone());
        if s.unavailable {
            return Err(RecoveryError::Unavailable);
        }
        let key = claim.work.request.key()?;
        let fence = claim.work.lease.as_ref().unwrap().fence;
        let mut target = claim.work.target.clone();
        if s.mismatch {
            target.conversation_id = "different-native".into();
        }
        let native_state = if target.active_turn.is_none() {
            ConfirmedNativeState::Idle
        } else {
            ConfirmedNativeState::Working
        };
        Ok((
            (key.clone(), fence),
            RecoveryConfirmation {
                operation_key: key,
                target,
                authority_fence: fence,
                native_state,
            },
        ))
    }

    async fn acknowledge(
        &self,
        _: Self::Attachment,
        completed: &RecoveryWork,
    ) -> Result<(), RecoveryError> {
        assert_eq!(completed.phase, RecoveryPhase::Completed);
        self.0.lock().unwrap().acknowledgements += 1;
        Ok(())
    }
}
