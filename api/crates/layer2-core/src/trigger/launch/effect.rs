use std::collections::BTreeMap;

use super::*;
use crate::{seconds_from_rfc3339, CapabilityRevocationState};

const INITIAL_PROMPT_TERMINAL_EVIDENCE_GRACE_SECONDS: u64 = 300;

pub(super) fn required_effect(launch: &Launch) -> Option<LaunchEffectKind> {
    match launch.state {
        LaunchState::Preflight => Some(LaunchEffectKind::Preflight),
        LaunchState::ForkPending => Some(LaunchEffectKind::Fork),
        LaunchState::ForkReady | LaunchState::BranchPending => Some(LaunchEffectKind::Branch),
        LaunchState::WorkspacePending => Some(LaunchEffectKind::WorkspaceImport),
        LaunchState::SandboxPending => Some(LaunchEffectKind::Sandbox),
        LaunchState::SessionPending => Some(LaunchEffectKind::Session),
        LaunchState::PromptPending if launch.reach.is_none() => Some(LaunchEffectKind::Reach),
        LaunchState::PromptPending => Some(LaunchEffectKind::InitialDispatch),
        LaunchState::Contributing
            if launch.diff.is_none()
                && launch
                    .contribution_checkpoint_version
                    .is_some_and(|requested| {
                        launch
                            .workspace
                            .as_ref()
                            .and_then(|workspace| workspace.latest_checkpoint.as_ref())
                            .is_none_or(|checkpoint| checkpoint.version < requested)
                    }) =>
        {
            Some(LaunchEffectKind::Checkpoint)
        }
        LaunchState::Contributing if launch.diff.is_none() => Some(LaunchEffectKind::Diff),
        LaunchState::PrPending if launch.contribution.is_none() => {
            Some(LaunchEffectKind::BranchPublication)
        }
        LaunchState::PrPending if launch.pull_request.is_none() => {
            Some(LaunchEffectKind::PullRequest)
        }
        _ => None,
    }
}

impl Launch {
    /// Claim the current durable effect after a worker lease has been committed.
    ///
    /// A prior claim, including one from an expired worker, is never silently
    /// executed again. It returns in reconciliation mode with the same intent.
    pub fn claim_next_effect(
        &mut self,
        lease: &WorkerLease,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<Option<EffectClaim>, TriggerValidationError> {
        let mut candidate = self.clone();
        let claim = candidate.apply_claim(lease, now.into(), correlation_id.into())?;
        *self = candidate;
        Ok(claim)
    }

    fn apply_claim(
        &mut self,
        lease: &WorkerLease,
        now: String,
        correlation_id: String,
    ) -> Result<Option<EffectClaim>, TriggerValidationError> {
        self.validate_worker_lease(lease, &now)?;
        if self.blocked_reason.is_some() || self.state.is_terminal() {
            return Ok(None);
        }

        if self.state == LaunchState::FailedRetryable {
            let failure = self.failure.as_ref().ok_or_else(|| {
                TriggerValidationError::field(
                    "failure",
                    "a retryable Launch must retain its failure classification",
                )
            })?;
            if !matches!(
                failure.class,
                FailureClass::RetryableTransient | FailureClass::ResourceQuota
            ) {
                return Ok(None);
            }
            let resume = self.resume_state.ok_or_else(|| {
                TriggerValidationError::field(
                    "resume_state",
                    "a retryable Launch must retain its durable step",
                )
            })?;
            let mut resumed = self.clone();
            resumed.state = resume;
            let Some(resumed_kind) = required_effect(&resumed) else {
                return Ok(None);
            };
            let Some(record) = self.effects.iter().rev().find(|record| {
                record.kind == resumed_kind && record.state == EffectState::FailedRetryable
            }) else {
                return Err(TriggerValidationError::field(
                    "effects",
                    "the retryable Launch has no matching effect record",
                ));
            };
            if record.attempt >= MAX_LAUNCH_EFFECT_ATTEMPTS
                || !deadline_reached(record.next_attempt_at.as_deref(), &now)?
            {
                return Ok(None);
            }
            self.transition(resume, now.clone(), correlation_id.clone())?;
            self.failure = None;
            self.resume_state = None;
        }

        let Some(kind) = required_effect(self) else {
            return Ok(None);
        };
        if kind == LaunchEffectKind::Branch && self.state == LaunchState::ForkReady {
            self.transition(
                LaunchState::BranchPending,
                now.clone(),
                correlation_id.clone(),
            )?;
        }

        let latest = self
            .effects
            .iter()
            .rposition(|record| record.kind == kind && record.state != EffectState::Succeeded);
        let (index, mode) = match latest {
            Some(index) => {
                let record = &self.effects[index];
                match record.state {
                    EffectState::Queued => (index, EffectClaimMode::Execute),
                    EffectState::FailedRetryable => {
                        if record.attempt >= MAX_LAUNCH_EFFECT_ATTEMPTS
                            || !deadline_reached(record.next_attempt_at.as_deref(), &now)?
                        {
                            return Ok(None);
                        }
                        (index, EffectClaimMode::Execute)
                    }
                    EffectState::Claimed | EffectState::Reconciling => {
                        if record.state == EffectState::Reconciling
                            && !deadline_reached(record.next_attempt_at.as_deref(), &now)?
                        {
                            return Ok(None);
                        }
                        (index, EffectClaimMode::Reconcile)
                    }
                    EffectState::RecoveryRequired
                    | EffectState::FailedTerminal
                    | EffectState::Cancelled
                    | EffectState::Succeeded => return Ok(None),
                }
            }
            None => {
                let ordinal = self
                    .effects
                    .iter()
                    .filter(|record| record.kind == kind)
                    .count()
                    + 1;
                let intent_key = format!(
                    "cma-trigger:{}:{}:{ordinal}",
                    self.id.0,
                    effect_kind_name(kind)
                );
                self.effects.push(EffectRecord {
                    kind,
                    state: EffectState::Queued,
                    intent_key,
                    attempt: 0,
                    reconciliation_attempt: 0,
                    fence: lease.fencing_token,
                    provider_request_id: None,
                    external_resource_ids: BTreeMap::new(),
                    next_attempt_at: None,
                    last_failure: None,
                    updated_at: now.clone(),
                });
                (self.effects.len() - 1, EffectClaimMode::Execute)
            }
        };

        let record = &mut self.effects[index];
        record.state = match mode {
            EffectClaimMode::Execute => EffectState::Claimed,
            EffectClaimMode::Reconcile => EffectState::Reconciling,
        };
        if mode == EffectClaimMode::Execute {
            record.attempt = record.attempt.checked_add(1).ok_or_else(|| {
                TriggerValidationError::field("effect.attempt", "attempt counter is exhausted")
            })?;
        } else {
            record.reconciliation_attempt = record
                .reconciliation_attempt
                .checked_add(1)
                .ok_or_else(|| {
                    TriggerValidationError::field(
                        "effect.reconciliation_attempt",
                        "reconciliation counter is exhausted",
                    )
                })?;
        }
        record.fence = lease.fencing_token;
        record.next_attempt_at = None;
        record.updated_at = now.clone();
        let claim = EffectClaim {
            kind,
            mode,
            intent_key: record.intent_key.clone(),
            attempt: record.attempt,
            reconciliation_attempt: record.reconciliation_attempt,
            fence: record.fence,
        };
        self.mark_step_running(&claim, &now, &correlation_id)?;
        if mode == EffectClaimMode::Reconcile {
            self.push_event(
                now,
                correlation_id,
                LaunchEventKind::EffectReconciled { effect: kind },
            )?;
        }
        Ok(Some(claim))
    }

    /// Mark a claimed effect successful after its typed durable result has
    /// already been attached to this candidate Launch.
    pub fn complete_effect(
        &mut self,
        lease: &WorkerLease,
        claim: &EffectClaim,
        result: LaunchEffectResult,
        safe_detail: impl Into<String>,
        external_resource_ids: BTreeMap<String, String>,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let mut candidate = self.clone();
        candidate.apply_completion(
            lease,
            claim,
            result,
            safe_detail.into(),
            external_resource_ids,
            now.into(),
            correlation_id.into(),
        )?;
        *self = candidate;
        Ok(())
    }

    /// Attach a nonterminal typed result while the same effect remains claimed.
    ///
    /// Initial prompt delivery is session-authoritative and can span several
    /// short worker leases. No other effect may publish partial result state.
    pub fn observe_effect_progress(
        &mut self,
        lease: &WorkerLease,
        claim: &EffectClaim,
        result: LaunchEffectResult,
        now: impl Into<String>,
        correlation_id: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let mut candidate = self.clone();
        let now = now.into();
        let correlation_id = correlation_id.into();
        candidate.validate_worker_lease(lease, &now)?;
        candidate.claimed_effect_index(claim)?;
        if claim.kind != LaunchEffectKind::InitialDispatch {
            return Err(TriggerValidationError::field(
                "effect_progress",
                "only initial prompt delivery has a partial result contract",
            ));
        }
        let LaunchEffectResult::InitialDispatch { delivery } = result else {
            return Err(TriggerValidationError::field(
                "effect_progress",
                "does not match initial prompt delivery",
            ));
        };
        candidate.observe_prompt_delivery(delivery, now, correlation_id)?;
        *self = candidate;
        Ok(())
    }

    /// Keep an unknown external outcome on its original intent and schedule a
    /// read-only reconciliation. This never authorizes repeating the write.
    pub fn defer_reconciliation(
        &mut self,
        lease: &WorkerLease,
        claim: &EffectClaim,
        mut failure: LaunchFailure,
        reconcile_at: String,
        external_resource_ids: BTreeMap<String, String>,
        now: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let mut candidate = self.clone();
        let now = now.into();
        candidate.validate_worker_lease(lease, &now)?;
        validate_failure(&failure)?;
        validate_external_resource_ids(&external_resource_ids)?;
        if !matches!(
            failure.class,
            FailureClass::AmbiguousExternal | FailureClass::RetryableTransient
        ) || !strictly_after(&reconcile_at, &now)?
        {
            return Err(TriggerValidationError::field(
                "reconciliation",
                "requires an ambiguous or transient outcome and a future deadline",
            ));
        }
        let index = candidate.claimed_effect_index(claim)?;
        let initial_prompt_deadline_reached = claim.kind == LaunchEffectKind::InitialDispatch
            && initial_prompt_observation_deadline_reached(&candidate, &now)?;
        let generic_budget_exhausted = claim.kind != LaunchEffectKind::InitialDispatch
            && candidate.effects[index].reconciliation_attempt
                >= MAX_LAUNCH_RECONCILIATION_ATTEMPTS;
        if generic_budget_exhausted || initial_prompt_deadline_reached {
            failure.class = FailureClass::RecoveryRequired;
            if initial_prompt_deadline_reached {
                failure.code = "initial_prompt_observation_deadline_exceeded".into();
                failure.detail =
                    "The initial task produced no terminal evidence before its session deadline."
                        .into();
                failure.remediation = "Review the retained session evidence before explicitly authorizing any replacement dispatch; the original task may still have executed.".into();
            } else {
                failure.code = "effect_reconciliation_budget_exhausted".into();
                failure.detail = "The external outcome could not be determined safely.".into();
                failure.remediation =
                    "Review provider evidence before authorizing any replacement effect.".into();
            }
            failure.retry_at = None;
            let record = &mut candidate.effects[index];
            record.state = EffectState::RecoveryRequired;
            record.next_attempt_at = None;
            record.last_failure = Some(failure.clone());
            record.updated_at = now.clone();
            candidate.mark_step_failed(claim, &failure, EffectState::RecoveryRequired, &now)?;
            candidate.fail(failure, now)?;
        } else {
            failure.retry_at = Some(reconcile_at.clone());
            let record = &mut candidate.effects[index];
            record.state = EffectState::Reconciling;
            record.next_attempt_at = Some(reconcile_at);
            record.last_failure = Some(failure.clone());
            record.provider_request_id = external_resource_ids
                .get("provider_request_id")
                .cloned()
                .or_else(|| record.provider_request_id.clone());
            record.external_resource_ids.extend(external_resource_ids);
            record.updated_at = now.clone();
            let step = candidate.step_mut(claim.kind)?;
            step.status = LaunchStepStatus::Retryable;
            step.safe_detail = "Checking the provider before continuing.".into();
            step.remediation = failure.remediation;
            step.worker_fencing_token = claim.fence;
            step.correlation_id = failure.correlation_id.clone();
            let step = step.clone();
            candidate.push_event(
                now,
                failure.correlation_id,
                LaunchEventKind::StepChanged { step },
            )?;
        }
        *self = candidate;
        Ok(())
    }

    fn apply_completion(
        &mut self,
        lease: &WorkerLease,
        claim: &EffectClaim,
        result: LaunchEffectResult,
        safe_detail: String,
        external_resource_ids: BTreeMap<String, String>,
        now: String,
        correlation_id: String,
    ) -> Result<(), TriggerValidationError> {
        self.validate_worker_lease(lease, &now)?;
        let delivery_before = self.delivery.clone();
        self.attach_effect_result(claim.kind, result)?;
        if claim.kind == LaunchEffectKind::InitialDispatch && self.delivery != delivery_before {
            let state = self
                .delivery
                .as_ref()
                .expect("an initial-dispatch result carries delivery state")
                .state;
            self.push_event(
                now.clone(),
                correlation_id.clone(),
                LaunchEventKind::DeliveryChanged { state },
            )?;
        }
        self.validate_effect_result(claim.kind, &external_resource_ids)?;
        let index = self.claimed_effect_index(claim)?;
        let record = &mut self.effects[index];
        record.state = EffectState::Succeeded;
        record.provider_request_id = external_resource_ids.get("provider_request_id").cloned();
        record.external_resource_ids = external_resource_ids.clone();
        record.next_attempt_at = None;
        record.last_failure = None;
        record.updated_at = now.clone();
        self.mark_step_completed(
            claim,
            safe_detail,
            external_resource_ids,
            &now,
            &correlation_id,
        )?;

        let target = match claim.kind {
            LaunchEffectKind::Preflight => Some(LaunchState::ForkPending),
            LaunchEffectKind::Fork => Some(LaunchState::ForkReady),
            LaunchEffectKind::Branch => Some(LaunchState::WorkspacePending),
            LaunchEffectKind::WorkspaceImport => Some(LaunchState::SandboxPending),
            LaunchEffectKind::Sandbox => Some(LaunchState::SessionPending),
            LaunchEffectKind::Session => Some(LaunchState::PromptPending),
            LaunchEffectKind::InitialDispatch => Some(LaunchState::Ready),
            LaunchEffectKind::PullRequest => Some(LaunchState::PrOpen),
            LaunchEffectKind::Reach
            | LaunchEffectKind::Checkpoint
            | LaunchEffectKind::Diff
            | LaunchEffectKind::BranchPublication
            | LaunchEffectKind::Cleanup => None,
        };
        if let Some(target) = target {
            self.transition(target, now, correlation_id)?;
        } else {
            self.updated_at = now;
        }
        Ok(())
    }

    fn observe_prompt_delivery(
        &mut self,
        delivery: PromptDelivery,
        now: String,
        correlation_id: String,
    ) -> Result<(), TriggerValidationError> {
        validate_prompt_delivery(self, &delivery)?;
        if let Some(previous) = self.delivery.as_ref() {
            if !same_prompt_identity(previous, &delivery)
                || delivery.attempt < previous.attempt
                || !legal_prompt_delivery_transition(previous.state, delivery.state)
            {
                return Err(TriggerValidationError::field(
                    "delivery",
                    "does not continue the retained initial prompt state machine",
                ));
            }
        }
        if self.delivery.as_ref() == Some(&delivery) {
            self.updated_at = now;
            return Ok(());
        }
        let state = delivery.state;
        self.delivery = Some(delivery);
        self.updated_at = now.clone();
        self.push_event(
            now,
            correlation_id,
            LaunchEventKind::DeliveryChanged { state },
        )
    }

    fn attach_effect_result(
        &mut self,
        kind: LaunchEffectKind,
        result: LaunchEffectResult,
    ) -> Result<(), TriggerValidationError> {
        match (kind, result) {
            (
                LaunchEffectKind::Preflight,
                LaunchEffectResult::Preflight {
                    connection,
                    authority,
                    source_sha,
                    workflow_paths,
                },
            ) => {
                connection.validate_for(
                    &self
                        .revision
                        .content
                        .repository_access
                        .required_scope_classes,
                )?;
                if authority.subject != self.owner
                    || authority.service_id != connection.service_id
                    || authority.detects_drift(&connection)
                {
                    return Err(TriggerValidationError::field(
                        "authority",
                        "does not match the current subject and connection",
                    ));
                }
                super::super::validate_sha("source_sha", &source_sha)?;
                self.connection = Some(connection);
                self.authority = Some(authority);
                self.source_sha = source_sha;
                self.workflow_paths = workflow_paths;
            }
            (LaunchEffectKind::Fork, LaunchEffectResult::Fork { fork }) => self.fork = Some(fork),
            (LaunchEffectKind::Branch, LaunchEffectResult::Branch { branch }) => {
                self.branch = Some(branch)
            }
            (
                LaunchEffectKind::WorkspaceImport,
                LaunchEffectResult::WorkspaceImport { workspace },
            ) => self.workspace = Some(workspace),
            (LaunchEffectKind::Sandbox, LaunchEffectResult::Sandbox) => {}
            (LaunchEffectKind::Session, LaunchEffectResult::Session { session }) => {
                self.session = Some(session)
            }
            (
                LaunchEffectKind::Reach,
                LaunchEffectResult::Reach {
                    reach,
                    connection,
                    authority,
                    branch,
                },
            ) => {
                reach.validate()?;
                connection.validate_for(
                    &self
                        .revision
                        .content
                        .repository_access
                        .required_scope_classes,
                )?;
                if authority.subject != self.owner
                    || authority.service_id != connection.service_id
                    || authority.detects_drift(&connection)
                    || reach.service_id != connection.service_id
                {
                    return Err(TriggerValidationError::field(
                        "reach",
                        "does not match the current subject and connection",
                    ));
                }
                self.connection = Some(connection);
                self.authority = Some(authority);
                self.reach = Some(reach);
                let retained_branch = self.branch.as_ref().ok_or_else(|| {
                    TriggerValidationError::field("branch", "the admitted policy has no branch")
                })?;
                if branch.name != retained_branch.name
                    || branch.expected_head != retained_branch.expected_head
                    || branch.canonical_url != retained_branch.canonical_url
                {
                    return Err(TriggerValidationError::field(
                        "branch",
                        "the admitted policy observed a different Launch branch",
                    ));
                }
                super::super::validate_sha("branch.current_head", &branch.current_head)?;
                self.branch = Some(branch);
            }
            (
                LaunchEffectKind::InitialDispatch,
                LaunchEffectResult::InitialDispatch { delivery },
            ) => self.delivery = Some(delivery),
            (LaunchEffectKind::Checkpoint, LaunchEffectResult::Checkpoint { checkpoint }) => {
                checkpoint.validate_complete(&self.id)?;
                let (session_id, minimum_runtime_generation) = self
                    .session
                    .as_ref()
                    .map(|session| (session.session_id.clone(), session.runtime_generation))
                    .ok_or_else(|| {
                        TriggerValidationError::field(
                            "checkpoint.session_id",
                            "cannot attach a checkpoint before the deterministic session",
                        )
                    })?;
                let workspace = self.workspace.as_mut().ok_or_else(|| {
                    TriggerValidationError::field(
                        "workspace",
                        "cannot attach a checkpoint before baseline import",
                    )
                })?;
                if checkpoint.workspace_epoch != workspace.workspace_epoch {
                    return Err(TriggerValidationError::field(
                        "checkpoint.workspace_epoch",
                        "does not match the current Launch workspace epoch",
                    ));
                }
                let expected_version = match workspace.latest_checkpoint.as_ref() {
                    None => 1,
                    Some(latest) => latest.version.checked_add(1).ok_or_else(|| {
                        TriggerValidationError::field(
                            "checkpoint.version",
                            "cannot advance beyond the retained checkpoint version",
                        )
                    })?,
                };
                if checkpoint.version != expected_version
                    || checkpoint.session_id != session_id
                    || checkpoint.runtime_generation < minimum_runtime_generation
                    || checkpoint.manifest.source_revision != self.source_sha
                    || checkpoint.delete_after.as_deref() != Some(workspace.delete_after.as_str())
                {
                    return Err(TriggerValidationError::field(
                        "checkpoint",
                        "does not match the Launch session, source, retention, and next version",
                    ));
                }
                let runtime_generation = checkpoint.runtime_generation;
                workspace.previous_checkpoint = workspace.latest_checkpoint.take();
                workspace.latest_checkpoint = Some(checkpoint);
                self.session
                    .as_mut()
                    .expect("validated above")
                    .runtime_generation = runtime_generation;
            }
            (LaunchEffectKind::Diff, LaunchEffectResult::Diff { diff }) => self.diff = Some(diff),
            (
                LaunchEffectKind::BranchPublication,
                LaunchEffectResult::BranchPublication { contribution },
            ) => self.contribution = Some(contribution),
            (LaunchEffectKind::PullRequest, LaunchEffectResult::PullRequest { pull_request }) => {
                self.pull_request = Some(pull_request)
            }
            (LaunchEffectKind::Cleanup, LaunchEffectResult::Cleanup) => {}
            _ => {
                return Err(TriggerValidationError::field(
                    "effect_result",
                    "does not match the claimed effect kind",
                ))
            }
        }
        Ok(())
    }

    /// Persist a classified effect failure. Unknown external outcomes must be
    /// reconciled by the adapter first; if they remain unknown this method
    /// holds the effect in `recovery_required` and never retries it blindly.
    pub fn fail_effect(
        &mut self,
        lease: &WorkerLease,
        claim: &EffectClaim,
        mut failure: LaunchFailure,
        next_attempt_at: Option<String>,
        now: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let mut candidate = self.clone();
        let now = now.into();
        candidate.validate_worker_lease(lease, &now)?;
        let index = candidate.claimed_effect_index(claim)?;
        validate_failure(&failure)?;

        let automatically_retryable = matches!(
            failure.class,
            FailureClass::RetryableTransient | FailureClass::ResourceQuota
        );
        let retry_at = if automatically_retryable {
            let retry_at = next_attempt_at.ok_or_else(|| {
                TriggerValidationError::field(
                    "effect.next_attempt_at",
                    "a bounded retry requires a persisted deadline",
                )
            })?;
            if !strictly_after(&retry_at, &now)? {
                return Err(TriggerValidationError::field(
                    "effect.next_attempt_at",
                    "must be later than the observed failure",
                ));
            }
            Some(retry_at)
        } else {
            None
        };

        if candidate.effects[index].attempt >= MAX_LAUNCH_EFFECT_ATTEMPTS && automatically_retryable
        {
            failure.class = FailureClass::RecoveryRequired;
            failure.code = "effect_retry_budget_exhausted".into();
            failure.detail = "The automatic retry budget was exhausted.".into();
            failure.remediation = "Review the retained effect evidence before retrying.".into();
            failure.retry_at = None;
        }

        let state = match failure.class {
            FailureClass::RetryableTransient | FailureClass::ResourceQuota => {
                EffectState::FailedRetryable
            }
            FailureClass::AmbiguousExternal | FailureClass::RecoveryRequired => {
                EffectState::RecoveryRequired
            }
            FailureClass::TerminalValidation
            | FailureClass::TerminalSecurity
            | FailureClass::PolicyUserAction => EffectState::FailedTerminal,
            FailureClass::AuthRequired
            | FailureClass::OwnerCredentialUnavailable
            | FailureClass::ServiceInactive
            | FailureClass::ConflictStaleHead
            | FailureClass::FeaturePaused => EffectState::FailedRetryable,
        };
        let effective_retry_at = (state == EffectState::FailedRetryable)
            .then_some(retry_at)
            .flatten();
        let record = &mut candidate.effects[index];
        record.state = state;
        record.next_attempt_at = effective_retry_at.clone();
        record.last_failure = Some(failure.clone());
        record.updated_at = now.clone();
        candidate.mark_step_failed(claim, &failure, state, &now)?;
        candidate.fail(failure, now)?;
        *self = candidate;
        Ok(())
    }

    fn validate_worker_lease(
        &self,
        lease: &WorkerLease,
        now: &str,
    ) -> Result<(), TriggerValidationError> {
        let current = self.lease.as_ref().ok_or_else(|| {
            TriggerValidationError::field("worker_lease", "is missing from the Launch")
        })?;
        if current != lease
            || lease.fencing_token != self.fencing_counter
            || !strictly_after(&lease.expires_at, now)?
        {
            return Err(TriggerValidationError::field(
                "worker_lease",
                "is expired or no longer owns the Launch fence",
            ));
        }
        Ok(())
    }

    fn claimed_effect_index(&self, claim: &EffectClaim) -> Result<usize, TriggerValidationError> {
        self.effects
            .iter()
            .position(|record| {
                record.kind == claim.kind
                    && record.intent_key == claim.intent_key
                    && record.attempt == claim.attempt
                    && record.fence == claim.fence
                    && matches!(
                        record.state,
                        EffectState::Claimed | EffectState::Reconciling
                    )
            })
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "effect_claim",
                    "is stale or does not name the currently claimed intent",
                )
            })
    }

    fn validate_effect_result(
        &self,
        kind: LaunchEffectKind,
        external_resource_ids: &BTreeMap<String, String>,
    ) -> Result<(), TriggerValidationError> {
        let present = |value: Option<&str>| value.is_some_and(|value| !value.trim().is_empty());
        match kind {
            LaunchEffectKind::Preflight => {
                if self.connection.is_none() || self.authority.is_none() {
                    return Err(TriggerValidationError::field(
                        "authority",
                        "preflight did not retain current authority evidence",
                    ));
                }
                super::super::validate_sha("source_sha", &self.source_sha)?;
            }
            LaunchEffectKind::Fork => {
                let fork = self.fork.as_ref().ok_or_else(|| {
                    TriggerValidationError::field("fork", "the reconciled fork is missing")
                })?;
                let upstream = &self.revision.content.upstream.repository;
                let expected_source = upstream.fork_source_id.unwrap_or(upstream.repository_id);
                if fork.source_revision != self.source_sha
                    || fork.repository.fork_parent_id != Some(upstream.repository_id)
                    || fork.repository.fork_source_id != Some(expected_source)
                    || self.connection.as_ref().is_none_or(|connection| {
                        connection.account.account_id != fork.repository.owner_id
                            || connection.account.account_node_id != fork.repository.owner_node_id
                    })
                {
                    return Err(TriggerValidationError::field(
                        "fork",
                        "does not match the Launch source, network, and connected owner",
                    ));
                }
            }
            LaunchEffectKind::Branch => {
                let branch = self.branch.as_ref().ok_or_else(|| {
                    TriggerValidationError::field("branch", "the fenced branch is missing")
                })?;
                super::super::validate_ref("branch.name", &branch.name)?;
                if branch.expected_head != self.source_sha || branch.current_head.trim().is_empty()
                {
                    return Err(TriggerValidationError::field(
                        "branch",
                        "does not retain the expected source head",
                    ));
                }
            }
            LaunchEffectKind::WorkspaceImport => {
                let workspace = self.workspace.as_ref().ok_or_else(|| {
                    TriggerValidationError::field("workspace", "the baseline import is missing")
                })?;
                workspace.baseline.validate()?;
                if workspace.baseline.source_revision != self.source_sha
                    || workspace.workspace_epoch == 0
                    || workspace.baseline_object_reference.trim().is_empty()
                    || workspace.baseline_archive_sha256.len() != 64
                    || !workspace
                        .baseline_archive_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                {
                    return Err(TriggerValidationError::field(
                        "workspace",
                        "does not match the Launch source and epoch",
                    ));
                }
            }
            LaunchEffectKind::Sandbox => {
                if !present(external_resource_ids.get("sandbox_id").map(String::as_str)) {
                    return Err(TriggerValidationError::field(
                        "effect.external_resource_ids.sandbox_id",
                        "is required",
                    ));
                }
            }
            LaunchEffectKind::Session => {
                if self.session.is_none() {
                    return Err(TriggerValidationError::field(
                        "session",
                        "the provisioned session link is missing",
                    ));
                }
            }
            LaunchEffectKind::Reach => {
                let reach = self.reach.as_ref().ok_or_else(|| {
                    TriggerValidationError::field("reach", "the admitted policy is missing")
                })?;
                reach.validate()?;
                if reach.state != CapabilityRevocationState::Active
                    || self
                        .connection
                        .as_ref()
                        .is_none_or(|connection| reach.service_id != connection.service_id)
                {
                    return Err(TriggerValidationError::field(
                        "reach",
                        "does not match the selected connection and policy generation",
                    ));
                }
            }
            LaunchEffectKind::InitialDispatch => {
                let delivery = self.delivery.as_ref().ok_or_else(|| {
                    TriggerValidationError::field("delivery", "the delivery record is missing")
                })?;
                if delivery.launch_id != self.id
                    || self
                        .session
                        .as_ref()
                        .is_none_or(|session| session.session_id != delivery.session_id)
                    || delivery.revision_digest != self.revision_digest
                    || delivery.state != PromptDeliveryState::Completed
                {
                    return Err(TriggerValidationError::field(
                        "delivery",
                        "is not a completed delivery for this Launch and session",
                    ));
                }
            }
            LaunchEffectKind::Checkpoint => {
                let checkpoint = self
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.latest_checkpoint.as_ref())
                    .ok_or_else(|| {
                        TriggerValidationError::field(
                            "checkpoint",
                            "the complete workspace checkpoint is missing",
                        )
                    })?;
                checkpoint.validate_complete(&self.id)?;
                let workspace = self.workspace.as_ref().expect("selected above");
                let session = self.session.as_ref().ok_or_else(|| {
                    TriggerValidationError::field(
                        "checkpoint.session_id",
                        "the complete checkpoint has no deterministic session",
                    )
                })?;
                if checkpoint.workspace_epoch != workspace.workspace_epoch
                    || checkpoint.session_id != session.session_id
                    || checkpoint.runtime_generation != session.runtime_generation
                    || checkpoint.manifest.source_revision != self.source_sha
                    || checkpoint.delete_after.as_deref() != Some(workspace.delete_after.as_str())
                {
                    return Err(TriggerValidationError::field(
                        "checkpoint",
                        "does not match the current Launch workspace and runtime",
                    ));
                }
            }
            LaunchEffectKind::Diff => {
                let diff = self.diff.as_ref().ok_or_else(|| {
                    TriggerValidationError::field("diff", "the captured diff is missing")
                })?;
                if self.contribution_checkpoint_version.is_none()
                    || self
                        .workspace
                        .as_ref()
                        .and_then(|workspace| workspace.latest_checkpoint.as_ref())
                        .is_none_or(|checkpoint| checkpoint.id != diff.checkpoint_id)
                {
                    return Err(TriggerValidationError::field(
                        "diff",
                        "does not match the explicitly requested checkpoint",
                    ));
                }
            }
            LaunchEffectKind::BranchPublication => {
                let contribution = self.contribution.as_ref().ok_or_else(|| {
                    TriggerValidationError::field(
                        "contribution",
                        "the branch publication record is missing",
                    )
                })?;
                contribution.validate_intent(&self.id)?;
                if contribution.resulting_head.is_none() {
                    return Err(TriggerValidationError::field(
                        "contribution.resulting_head",
                        "publication has not reached the guarded ref",
                    ));
                }
            }
            LaunchEffectKind::PullRequest => {
                let pull_request = self.pull_request.as_ref().ok_or_else(|| {
                    TriggerValidationError::field(
                        "pull_request",
                        "the reconciled pull request is missing",
                    )
                })?;
                pull_request.validate_intent(&self.id)?;
                if pull_request.number == 0 {
                    return Err(TriggerValidationError::field(
                        "pull_request.number",
                        "the provider PR number is missing",
                    ));
                }
            }
            LaunchEffectKind::Cleanup => {}
        }
        validate_external_resource_ids(external_resource_ids)
    }

    fn mark_step_running(
        &mut self,
        claim: &EffectClaim,
        now: &str,
        correlation_id: &str,
    ) -> Result<(), TriggerValidationError> {
        let step = self.step_mut(claim.kind)?;
        step.status = LaunchStepStatus::Running;
        step.started_at.get_or_insert_with(|| now.to_string());
        step.completed_at = None;
        step.attempt = claim.attempt;
        step.safe_detail = match claim.mode {
            EffectClaimMode::Execute => "Working on this step.".into(),
            EffectClaimMode::Reconcile => "Checking a prior attempt before continuing.".into(),
        };
        step.remediation.clear();
        step.worker_fencing_token = claim.fence;
        step.correlation_id = correlation_id.into();
        let step = step.clone();
        self.updated_at = now.into();
        self.push_event(now, correlation_id, LaunchEventKind::StepChanged { step })
    }

    fn mark_step_completed(
        &mut self,
        claim: &EffectClaim,
        safe_detail: String,
        external_resource_ids: BTreeMap<String, String>,
        now: &str,
        correlation_id: &str,
    ) -> Result<(), TriggerValidationError> {
        let step = self.step_mut(claim.kind)?;
        step.status = LaunchStepStatus::Completed;
        step.completed_at = Some(now.into());
        step.safe_detail = safe_detail;
        step.remediation.clear();
        step.external_resource_ids = external_resource_ids;
        step.worker_fencing_token = claim.fence;
        step.correlation_id = correlation_id.into();
        let step = step.clone();
        self.push_event(now, correlation_id, LaunchEventKind::StepChanged { step })
    }

    fn mark_step_failed(
        &mut self,
        claim: &EffectClaim,
        failure: &LaunchFailure,
        state: EffectState,
        now: &str,
    ) -> Result<(), TriggerValidationError> {
        let step = self.step_mut(claim.kind)?;
        step.status = match state {
            EffectState::FailedRetryable => LaunchStepStatus::Retryable,
            EffectState::RecoveryRequired => LaunchStepStatus::Blocked,
            _ => LaunchStepStatus::Failed,
        };
        step.safe_detail = failure.detail.clone();
        step.remediation = failure.remediation.clone();
        step.worker_fencing_token = claim.fence;
        step.correlation_id = failure.correlation_id.clone();
        let step = step.clone();
        self.push_event(
            now,
            failure.correlation_id.clone(),
            LaunchEventKind::StepChanged { step },
        )
    }

    fn step_mut(
        &mut self,
        kind: LaunchEffectKind,
    ) -> Result<&mut LaunchStep, TriggerValidationError> {
        let key = step_key(kind);
        self.steps
            .iter_mut()
            .find(|step| step.key == key)
            .ok_or_else(|| {
                TriggerValidationError::field("steps", "the stable effect step is missing")
            })
    }
}

fn initial_prompt_observation_deadline_reached(
    launch: &Launch,
    now: &str,
) -> Result<bool, TriggerValidationError> {
    let delivery = launch.delivery.as_ref().ok_or_else(|| {
        TriggerValidationError::field(
            "delivery",
            "initial prompt reconciliation requires retained delivery progress",
        )
    })?;
    let deadline = timestamp(&delivery.queued_at)?
        .checked_add(u64::from(launch.effective.timeout_seconds))
        .and_then(|value| value.checked_add(INITIAL_PROMPT_TERMINAL_EVIDENCE_GRACE_SECONDS))
        .ok_or_else(|| {
            TriggerValidationError::field(
                "delivery.queued_at",
                "initial prompt observation deadline overflows",
            )
        })?;
    Ok(timestamp(now)? >= deadline)
}

fn step_key(kind: LaunchEffectKind) -> LaunchStepKey {
    match kind {
        LaunchEffectKind::Preflight => LaunchStepKey::Preflight,
        LaunchEffectKind::Fork => LaunchStepKey::Fork,
        LaunchEffectKind::Branch => LaunchStepKey::Branch,
        LaunchEffectKind::WorkspaceImport => LaunchStepKey::Workspace,
        LaunchEffectKind::Sandbox => LaunchStepKey::Sandbox,
        LaunchEffectKind::Session => LaunchStepKey::Session,
        LaunchEffectKind::Reach => LaunchStepKey::Reach,
        LaunchEffectKind::InitialDispatch => LaunchStepKey::InitialPrompt,
        LaunchEffectKind::Checkpoint => LaunchStepKey::Checkpoint,
        LaunchEffectKind::Diff => LaunchStepKey::Diff,
        LaunchEffectKind::BranchPublication => LaunchStepKey::PublishBranch,
        LaunchEffectKind::PullRequest => LaunchStepKey::PullRequest,
        LaunchEffectKind::Cleanup => LaunchStepKey::Cleanup,
    }
}

fn effect_kind_name(kind: LaunchEffectKind) -> &'static str {
    match kind {
        LaunchEffectKind::Preflight => "preflight",
        LaunchEffectKind::Fork => "fork",
        LaunchEffectKind::Branch => "branch",
        LaunchEffectKind::WorkspaceImport => "workspace_import",
        LaunchEffectKind::Sandbox => "sandbox",
        LaunchEffectKind::Session => "session",
        LaunchEffectKind::Reach => "reach",
        LaunchEffectKind::InitialDispatch => "initial_dispatch",
        LaunchEffectKind::Checkpoint => "checkpoint",
        LaunchEffectKind::Diff => "diff",
        LaunchEffectKind::BranchPublication => "branch_publication",
        LaunchEffectKind::PullRequest => "pull_request",
        LaunchEffectKind::Cleanup => "cleanup",
    }
}

fn validate_prompt_delivery(
    launch: &Launch,
    delivery: &PromptDelivery,
) -> Result<(), TriggerValidationError> {
    if delivery.launch_id != launch.id
        || launch
            .session
            .as_ref()
            .is_none_or(|session| session.session_id != delivery.session_id)
        || delivery.revision_digest != launch.revision_digest
        || delivery.origin_key
            != format!(
                "cma-trigger:{}:{}:initial",
                launch.id.0, launch.revision_digest
            )
        || delivery.payload_hash.len() != 64
        || !delivery
            .payload_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || crate::seconds_from_rfc3339(&delivery.queued_at).is_none()
        || crate::seconds_from_rfc3339(&delivery.updated_at).is_none()
        || crate::seconds_from_rfc3339(&delivery.updated_at)
            < crate::seconds_from_rfc3339(&delivery.queued_at)
        || (delivery.state != PromptDeliveryState::Queued && delivery.attempt == 0)
    {
        return Err(TriggerValidationError::field(
            "delivery",
            "does not identify a valid prompt for this Launch and session",
        ));
    }
    let requires_fence = delivery.state != PromptDeliveryState::Queued;
    if requires_fence
        != delivery
            .dispatch_fence_sequence
            .is_some_and(|sequence| sequence > 0)
    {
        return Err(TriggerValidationError::field(
            "delivery.dispatch_fence_sequence",
            "must be present exactly after the dispatch fence is committed",
        ));
    }
    Ok(())
}

fn same_prompt_identity(left: &PromptDelivery, right: &PromptDelivery) -> bool {
    left.launch_id == right.launch_id
        && left.session_id == right.session_id
        && left.revision_digest == right.revision_digest
        && left.origin_key == right.origin_key
        && left.payload_hash == right.payload_hash
        && left.queued_at == right.queued_at
}

fn legal_prompt_delivery_transition(from: PromptDeliveryState, to: PromptDeliveryState) -> bool {
    use PromptDeliveryState as State;
    from == to
        || matches!(
            (from, to),
            (State::Queued, State::DispatchFenced)
                | (State::Queued, State::WriteConfirmed)
                | (State::Queued, State::ExecutionObserved)
                | (State::Queued, State::Completed)
                | (State::Queued, State::Failed)
                | (State::Queued, State::RecoveryRequired)
                | (State::DispatchFenced, State::Queued)
                | (State::DispatchFenced, State::WriteConfirmed)
                | (State::DispatchFenced, State::ExecutionObserved)
                | (State::DispatchFenced, State::Completed)
                | (State::DispatchFenced, State::Failed)
                | (State::DispatchFenced, State::RecoveryRequired)
                | (State::WriteConfirmed, State::ExecutionObserved)
                | (State::WriteConfirmed, State::Completed)
                | (State::WriteConfirmed, State::Failed)
                | (State::WriteConfirmed, State::RecoveryRequired)
                | (State::ExecutionObserved, State::Completed)
                | (State::ExecutionObserved, State::Failed)
                | (State::RecoveryRequired, State::ExecutionObserved)
                | (State::RecoveryRequired, State::Completed)
                | (State::RecoveryRequired, State::Failed)
        )
}

fn deadline_reached(deadline: Option<&str>, now: &str) -> Result<bool, TriggerValidationError> {
    match deadline {
        Some(deadline) => Ok(timestamp(deadline)? <= timestamp(now)?),
        None => Ok(false),
    }
}

fn strictly_after(later: &str, earlier: &str) -> Result<bool, TriggerValidationError> {
    Ok(timestamp(later)? > timestamp(earlier)?)
}

fn timestamp(value: &str) -> Result<u64, TriggerValidationError> {
    seconds_from_rfc3339(value).ok_or_else(|| {
        TriggerValidationError::field("timestamp", "must use CMA's canonical UTC format")
    })
}

fn validate_failure(failure: &LaunchFailure) -> Result<(), TriggerValidationError> {
    if failure.code.is_empty()
        || failure.code.len() > 128
        || failure
            .code
            .bytes()
            .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && byte != b'_')
        || failure.detail.is_empty()
        || failure.detail.len() > 1_024
        || failure.remediation.is_empty()
        || failure.remediation.len() > 1_024
        || failure.correlation_id.is_empty()
        || failure.correlation_id.len() > 256
        || timestamp(&failure.observed_at).is_err()
    {
        return Err(TriggerValidationError::field(
            "failure",
            "contains unsafe or malformed public detail",
        ));
    }
    Ok(())
}

fn validate_external_resource_ids(
    external_resource_ids: &BTreeMap<String, String>,
) -> Result<(), TriggerValidationError> {
    if external_resource_ids.len() > 32 {
        return Err(TriggerValidationError::field(
            "effect.external_resource_ids",
            "contains too many entries",
        ));
    }
    for (key, value) in external_resource_ids {
        if key.is_empty()
            || key.len() > 128
            || value.trim().is_empty()
            || value.len() > 1_024
            || key
                .bytes()
                .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'_')
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(TriggerValidationError::field(
                "effect.external_resource_ids",
                "contains an unsafe key or value",
            ));
        }
    }
    Ok(())
}
