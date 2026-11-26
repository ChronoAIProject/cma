use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{SandboxLifecycleStore, SandboxStoreError, SandboxTransaction};
use layer2_core::sandbox::work_retry::WorkRetryFailure;
use layer2_core::sandbox::{
    Sandbox, SandboxCredentialAuthority, SandboxEventKind, SandboxStatus, SandboxWorkStep,
    TypedFailure,
};
use layer2_core::{rfc3339_at, seconds_from_rfc3339};
use layer3_adapter::{AdapterError, RuntimeHandle};

use crate::sandbox::credential::{CredentialError, CredentialMaterial};
use crate::sandbox::funding::FundingError;

use super::{
    SandboxDependencyError, SandboxWorker, CREDENTIAL_EXPIRY_MARGIN, CREDENTIAL_ROTATION_AGE,
};

impl SandboxWorker {
    pub(super) async fn advance(
        &self,
        sandbox: Sandbox,
        next: SandboxWorkStep,
        now: SystemTime,
        event: SandboxEventKind,
    ) -> Result<(), SandboxStoreError> {
        self.advance_at(sandbox, next, now, now, event).await
    }

    pub(super) async fn advance_at(
        &self,
        mut sandbox: Sandbox,
        next: SandboxWorkStep,
        now: SystemTime,
        ready_at: SystemTime,
        event: SandboxEventKind,
    ) -> Result<(), SandboxStoreError> {
        let expected_revision = sandbox.revision;
        let fence = self.claimed_fence(&sandbox)?;
        sandbox
            .work
            .advance(&self.config.holder, fence, next, rfc3339_at(ready_at))
            .map_err(|_| SandboxStoreError::Conflict)?;
        sandbox.revision = expected_revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        let event_at = rfc3339_at(now);
        sandbox.updated_at = event_at.clone();
        commit_advance(
            &self.dependencies.store,
            expected_revision,
            sandbox,
            event_at,
            event,
        )
        .await
    }

    pub(super) async fn advance_status(
        &self,
        sandbox: Sandbox,
        status: SandboxStatus,
        next: SandboxWorkStep,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        self.advance_status_at(sandbox, status, next, now, now)
            .await
    }

    pub(super) async fn advance_status_at(
        &self,
        mut sandbox: Sandbox,
        status: SandboxStatus,
        next: SandboxWorkStep,
        now: SystemTime,
        ready_at: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let expected_revision = sandbox.revision;
        let previous = sandbox.status;
        let fence = self.claimed_fence(&sandbox)?;
        sandbox
            .work
            .advance(&self.config.holder, fence, next, rfc3339_at(ready_at))
            .map_err(|_| SandboxStoreError::Conflict)?;
        let event_at = rfc3339_at(now);
        sandbox
            .transition(status, event_at.clone(), None)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        commit_advance(
            &self.dependencies.store,
            expected_revision,
            sandbox,
            event_at,
            SandboxEventKind::StatusChanged {
                from: previous,
                to: status,
            },
        )
        .await
    }

    pub(super) async fn handle_failure(
        &self,
        mut sandbox: Sandbox,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        if failure.retryable {
            return self.retry_failure(sandbox, failure, now).await;
        }
        let expected_revision = sandbox.revision;
        let fence = self.claimed_fence(&sandbox)?;
        let event_at = rfc3339_at(now);
        sandbox
            .work
            .advance(
                &self.config.holder,
                fence,
                SandboxWorkStep::None,
                event_at.clone(),
            )
            .map_err(|_| SandboxStoreError::Conflict)?;
        let typed = failure.typed();
        sandbox
            .transition(SandboxStatus::Failed, event_at.clone(), Some(typed.clone()))
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        commit_advance(
            &self.dependencies.store,
            expected_revision,
            sandbox,
            event_at,
            SandboxEventKind::Failed { failure: typed },
        )
        .await
    }

    pub(super) async fn retry_failure(
        &self,
        mut sandbox: Sandbox,
        failure: WorkerFailure,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let decision = self.retry_decision(&sandbox, &failure, self.config.retry_delay);
        let telemetry = self.retry_telemetry(&sandbox, &failure, decision, now);
        let native_retry = self.native_retry_class(&sandbox, &failure);
        let expected_revision = sandbox.revision;
        let step = sandbox.work.step;
        let fence = self.claimed_fence(&sandbox)?;
        sandbox
            .work
            .retry(&self.config.holder, fence, rfc3339_at(now + decision.delay))
            .map_err(|_| SandboxStoreError::Conflict)?;
        sandbox.revision = expected_revision
            .checked_add(1)
            .ok_or(SandboxStoreError::Conflict)?;
        let event_at = rfc3339_at(now);
        sandbox.updated_at = event_at.clone();
        commit_advance(
            &self.dependencies.store,
            expected_revision,
            sandbox,
            event_at,
            SandboxEventKind::WorkStepFailed {
                step,
                failure: failure.typed(),
            },
        )
        .await?;
        super::retry::native_retry_published(native_retry);
        if let Some(telemetry) = telemetry {
            telemetry.published();
        }
        Ok(())
    }

    fn claimed_fence(&self, sandbox: &Sandbox) -> Result<u64, SandboxStoreError> {
        sandbox
            .work
            .lease
            .as_ref()
            .filter(|lease| lease.holder == self.config.holder)
            .map(|lease| lease.fence)
            .ok_or(SandboxStoreError::Conflict)
    }
}

async fn commit_advance(
    store: &std::sync::Arc<dyn SandboxLifecycleStore>,
    expected_revision: u64,
    sandbox: Sandbox,
    event_at: String,
    event: SandboxEventKind,
) -> Result<(), SandboxStoreError> {
    sandbox.validate().map_err(|error| {
        SandboxStoreError::Corrupt(format!("advanced sandbox is invalid: {error}"))
    })?;
    store
        .commit(SandboxTransaction::SaveSandboxAndAppend {
            command_revival: None,
            released_runtime: None,
            expected_revision,
            sandbox: Box::new(sandbox),
            event_at,
            event,
        })
        .await
}

pub(super) fn runtime(sandbox: &Sandbox) -> Option<RuntimeHandle> {
    sandbox
        .runtime
        .as_ref()
        .map(|binding| RuntimeHandle(binding.runtime_id.clone()))
}

pub(super) fn operation_id(sandbox: &Sandbox) -> String {
    if sandbox.work.authority_protocol.is_some() {
        // A new claimant continues the same effect. Runtime and credential
        // generations distinguish later obligations without using lease fences.
        return format!(
            "sandbox-work-v1:{}:{:?}:{}:{}",
            sandbox.id,
            sandbox.work.step,
            sandbox.lifecycle.runtime_generation_high_water,
            sandbox.credential.key_generation
        );
    }
    let fence = sandbox.work.lease.as_ref().map_or(0, |lease| lease.fence);
    format!("{}:{:?}:{fence}", sandbox.id, sandbox.work.step)
}

pub(super) fn material_matches(
    sandbox: &Sandbox,
    pending: &SandboxCredentialAuthority,
    material: &CredentialMaterial,
) -> bool {
    material.sandbox_id == sandbox.id
        && material.owner == sandbox.owner
        && material.source == sandbox.credential.source
        && material.matches_source()
        && material.matches_connection(sandbox)
        && &material.authority == pending
}

pub(super) fn credential_valid_for_delivery(sandbox: &Sandbox, now: SystemTime) -> bool {
    if !sandbox.credential.source.is_managed() {
        return sandbox
            .credential
            .pending
            .as_ref()
            .is_some_and(|pending| sandbox.credential.source.accepts(pending));
    }
    let now = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    sandbox
        .credential
        .pending
        .as_ref()
        .and_then(|pending| seconds_from_rfc3339(&pending.expires_at))
        .is_some_and(|expires| expires > now.saturating_add(CREDENTIAL_EXPIRY_MARGIN.as_secs()))
}

pub(super) fn credential_rotation_at(sandbox: &Sandbox) -> Option<SystemTime> {
    if !sandbox.credential.source.is_managed() {
        return None;
    }
    let current = sandbox.credential.current.as_ref()?;
    let issued = seconds_from_rfc3339(&current.issued_at)?;
    let expires = seconds_from_rfc3339(&current.expires_at)?;
    let age_due = issued.checked_add(CREDENTIAL_ROTATION_AGE.as_secs())?;
    let expiry_due = expires.checked_sub(CREDENTIAL_EXPIRY_MARGIN.as_secs())?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(age_due.min(expiry_due)))
}

pub(super) fn system_time(stamp: &str) -> Option<SystemTime> {
    seconds_from_rfc3339(stamp).map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
}

#[derive(Debug, Clone)]
pub(super) struct WorkerFailure {
    class: &'static str,
    detail: String,
    retryable: bool,
    retry_kind: WorkRetryFailure,
    pub(super) native_claim_pending: bool,
    pub(super) startup_hold: bool,
    pub(super) credential_issuance_pending: bool,
    pub(super) reopen_refusal: Option<Box<layer2_core::sandbox::store::ReopenRefusalFence>>,
}

impl WorkerFailure {
    pub(super) fn retryable(detail: &str) -> Self {
        Self::new("retryable_transient", detail, true)
    }

    pub(super) fn waiting(detail: &str) -> Self {
        Self::retryable(detail).with_retry_kind(WorkRetryFailure::Waiting)
    }

    pub(super) fn with_retry_kind(mut self, kind: WorkRetryFailure) -> Self {
        self.retry_kind = kind;
        self
    }

    pub(super) fn retry_kind(&self) -> WorkRetryFailure {
        self.retry_kind
    }

    fn resource_quota(detail: &str) -> Self {
        Self::new("resource_quota", detail, false)
    }

    pub(super) fn terminal_validation(detail: &str) -> Self {
        Self::new("terminal_validation", detail, false)
    }

    pub(super) fn terminal_security(detail: &str) -> Self {
        Self::new("terminal_security", detail, false)
    }

    fn service_not_granted(service: &str) -> Self {
        Self::new("service_not_granted", service, false)
    }

    pub(super) fn new(class: &'static str, detail: &str, retryable: bool) -> Self {
        Self {
            class,
            detail: safe_detail(detail),
            retryable,
            native_claim_pending: false,
            startup_hold: false,
            credential_issuance_pending: false,
            reopen_refusal: None,
            retry_kind: if retryable {
                WorkRetryFailure::Transient
            } else {
                WorkRetryFailure::ExternalAction
            },
        }
    }

    pub(super) fn is_retryable(&self) -> bool {
        self.retryable
    }

    pub(super) fn detail(&self) -> &str {
        &self.detail
    }

    pub(super) fn typed(&self) -> TypedFailure {
        TypedFailure {
            class: self.class.into(),
            detail: self.detail.clone(),
        }
    }
}

pub(super) fn adapter_failure(error: AdapterError) -> WorkerFailure {
    tracing::warn!(error = %error, "agent-sandbox runtime operation failed");
    let external = matches!(&error, AdapterError::Unavailable(detail)
        if crate::sandbox::work_retry::replaced_runtime(detail));
    match error {
        AdapterError::AccessRevoked => {
            WorkerFailure::terminal_security("current runtime authority was revoked")
        }
        AdapterError::InputAuthorityUnavailable(_)
        | AdapterError::PromptNotWritten(_)
        | AdapterError::NativePoolDefinitionAbsent { .. }
        | AdapterError::Unavailable(_) => WorkerFailure::retryable(
            "runtime dependency unavailable",
        )
        .with_retry_kind(if external {
            WorkRetryFailure::ExternalAction
        } else {
            WorkRetryFailure::Transient
        }),
        AdapterError::NativeClaimNotReady(reason) => {
            use layer3_adapter::agent_sandbox::NativeClaimGuardReason;
            let detail = match reason {
                NativeClaimGuardReason::Unschedulable => {
                    "runtime pod could not be scheduled; original identity retained"
                }
                _ => "native claim is not ready; original identity retained",
            };
            let mut failure = WorkerFailure::retryable(detail);
            failure.native_claim_pending = reason == NativeClaimGuardReason::Pending;
            failure
        }
        AdapterError::RuntimeStartupHold(failure) => WorkerFailure::startup_hold(failure.class),
        AdapterError::RuntimeLedgerHold(_) => {
            WorkerFailure::retryable("runtime mutation ledger capacity requires recovery")
        }
        AdapterError::RuntimeCapacity(_) => WorkerFailure::retryable(
            "runtime_request_capacity_exhausted; admitted operation retained",
        ),
        AdapterError::AtCapacity(_) => {
            WorkerFailure::resource_quota("requested sandbox capacity is unavailable")
        }
        AdapterError::Rejected(_)
        | AdapterError::AdmittedSandboxSpecDrift
        | AdapterError::NativePoolConfigurationDrift
        | AdapterError::DesktopFailed
        | AdapterError::SkillUpgradeRefused(_)
        | AdapterError::OperationIdentityConflict(_)
        | AdapterError::Unsupported(_) => {
            WorkerFailure::terminal_validation("runtime rejected the lifecycle operation")
        }
        AdapterError::RuntimeContractMismatch(_) => {
            WorkerFailure::terminal_security("runtime image contract was not admitted")
        }
    }
}

pub(super) use super::install_failure::install_failure;

pub(super) fn credential_failure(error: CredentialError) -> WorkerFailure {
    match error {
        CredentialError::Uncertain => {
            let mut failure = WorkerFailure::waiting(
                "credential issuance is uncertain; reconciliation will retry",
            );
            failure.credential_issuance_pending = true;
            failure
        }
        CredentialError::ServiceNotGranted(service) => {
            WorkerFailure::service_not_granted(service.as_str())
        }
        CredentialError::Unavailable(_) => {
            WorkerFailure::retryable("credential dependency unavailable")
        }

        CredentialError::Security(_) => {
            WorkerFailure::terminal_security("credential authority was refused")
        }
    }
}

pub(super) fn dependency_failure(error: SandboxDependencyError) -> WorkerFailure {
    match error {
        SandboxDependencyError::AgentsStopping => {
            WorkerFailure::retryable("lifecycle dependency unavailable")
                .with_retry_kind(WorkRetryFailure::Waiting)
        }
        SandboxDependencyError::Transient(detail) => WorkerFailure::retryable(
            "lifecycle dependency unavailable",
        )
        .with_retry_kind(if detail == "sandbox agents are still stopping" {
            WorkRetryFailure::Waiting
        } else {
            WorkRetryFailure::Transient
        }),
        SandboxDependencyError::ResourceQuota(_) => {
            WorkerFailure::resource_quota("lifecycle dependency has no available capacity")
        }
        SandboxDependencyError::Validation(_) => {
            WorkerFailure::terminal_validation("lifecycle dependency rejected the operation")
        }
        SandboxDependencyError::Security(_) => {
            WorkerFailure::terminal_security("lifecycle dependency refused its authority")
        }
    }
}

pub(super) fn funding_failure(error: FundingError) -> WorkerFailure {
    match error {
        FundingError::Conflict | FundingError::NotFound | FundingError::RateLimited(_) => {
            WorkerFailure::retryable("prepaid funding recovery is pending")
        }
        FundingError::Retryable(detail) => {
            WorkerFailure::retryable(&format!("prepaid Sandbox funding unavailable: {detail}"))
        }
        FundingError::NotEnrolled => WorkerFailure::resource_quota(
            "paid admission closed; existing funding must finish recovery and settlement",
        ),
        FundingError::Unfunded(detail) => WorkerFailure::resource_quota(&format!(
            "prepaid Sandbox window was not funded: {detail}"
        )),
        FundingError::Invalid(detail) => WorkerFailure::terminal_security(&format!(
            "prepaid Sandbox funding contract invalid: {detail}"
        )),
    }
}

fn safe_detail(detail: &str) -> String {
    let filtered = detail
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect::<String>();
    if filtered.trim().is_empty() {
        "lifecycle operation failed".into()
    } else {
        filtered
    }
}

#[cfg(test)]
mod claim_guard_tests;
