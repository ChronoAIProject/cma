use std::{collections::BTreeSet, time::SystemTime};

use layer2_core::{
    rfc3339_at,
    sandbox::{store::WorkAuthority, Sandbox, SandboxStoreError},
};

use super::{
    support::{credential_failure, WorkerFailure},
    SandboxWorker,
};

impl SandboxWorker {
    /// A provider delete can collect the home before CMA observes its reply.
    /// Publish settlement under the existing claim before dispatching that effect.
    pub(super) async fn prepare_retirement_cleanup(
        &self,
        sandbox: &mut Sandbox,
        now: SystemTime,
    ) -> Result<Option<WorkerFailure>, SandboxStoreError> {
        if let Err(failure) = self.settle_retirement_credentials(sandbox, now).await {
            return Ok(Some(failure));
        }
        let expected = sandbox.revision;
        let mut settled = sandbox.clone();
        settled.revision = expected.checked_add(1).ok_or(SandboxStoreError::Conflict)?;
        settled.updated_at = settled.updated_at.max(rfc3339_at(now));
        self.dependencies
            .store
            .save_sandbox(expected, settled.clone())
            .await?;
        // An unresolved commit or a lost claim must never become local permission
        // to delete. A successor repeats the census and the idempotent revocations.
        WorkAuthority::current()?;
        *sandbox = settled;
        Ok(None)
    }

    pub(super) async fn settle_retirement_credentials(
        &self,
        sandbox: &mut Sandbox,
        now: SystemTime,
    ) -> Result<(), WorkerFailure> {
        let before = sandbox.credential.clone();
        let result = self.reconcile_retirement_credentials(sandbox, now).await;
        if result
            .as_ref()
            .is_err_and(|failure| failure.credential_issuance_pending)
        {
            // The final issuance check can fail after local revocations. Keep
            // the claimed parent intact for the compute suspension fence.
            sandbox.credential = before;
        }
        result
    }

    async fn reconcile_retirement_credentials(
        &self,
        sandbox: &mut Sandbox,
        now: SystemTime,
    ) -> Result<(), WorkerFailure> {
        let credentials = &self.dependencies.credentials;
        credentials
            .settle_revocations(sandbox)
            .await
            .map_err(credential_failure)?;
        let custody = credentials
            .custodied(&sandbox.id)
            .await
            .map_err(credential_failure)?;
        let mut ids = custody_provider_ids(sandbox, &custody)?;
        ids.extend(
            sandbox
                .credential
                .current
                .iter()
                .chain(&sandbox.credential.pending)
                .map(|authority| authority.provider_key_id.clone()),
        );
        ids.extend(
            sandbox
                .credential
                .revoke_debt
                .iter()
                .map(|debt| debt.provider_key_id.clone()),
        );
        for id in &ids {
            sandbox.credential.queue_revoke(id.clone(), rfc3339_at(now));
        }
        let mut failure = None;
        let can_recover = credentials.uses_retirement_reauthorization(sandbox);
        for id in ids {
            let result = async {
                credentials.revoke(sandbox, &id).await?;
                credentials.forget_provider(&sandbox.id, &id).await
            }
            .await;
            match result {
                Ok(()) => sandbox
                    .credential
                    .revoke_debt
                    .retain(|debt| debt.provider_key_id != id),
                Err(error) if can_recover => {
                    crate::sandbox::credential::retain_strongest_refusal(&mut failure, error)
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        sandbox.credential.current = None;
        sandbox.credential.pending = None;
        sandbox.credential.delivered = false;
        if let Some(failure) = failure {
            return Err(credential_failure(failure));
        }
        credentials
            .settle_revocations(sandbox)
            .await
            .map_err(credential_failure)?;
        let remaining = credentials
            .custodied(&sandbox.id)
            .await
            .map_err(credential_failure)?;
        let ids = custody_provider_ids(sandbox, &remaining)?;
        for id in ids {
            sandbox.credential.queue_revoke(id, rfc3339_at(now));
        }
        if !sandbox.credential.revoke_debt.is_empty() {
            return Err(WorkerFailure::waiting(
                "credential cleanup remains before home removal",
            ));
        }
        Ok(())
    }
}

pub(super) fn custody_provider_ids(
    sandbox: &Sandbox,
    custodied: &[crate::sandbox::credential::CredentialMaterial],
) -> Result<BTreeSet<String>, WorkerFailure> {
    if custodied.iter().any(|material| {
        material.sandbox_id != sandbox.id
            || material.owner != sandbox.owner
            || material.source != sandbox.credential.source
            || !material.matches_source()
    }) {
        return Err(WorkerFailure::terminal_security(
            "credential custody crossed its sandbox boundary",
        ));
    }
    Ok(custodied
        .iter()
        .map(|material| material.authority.provider_key_id.clone())
        .collect())
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OwnerReauthorization {
    pub previous: layer2_core::sandbox::RetirementAuthorityVersion,
    pub missing: bool,
}

pub(super) fn park_reauthorization(
    sandbox: &mut Sandbox,
    authority: Option<OwnerReauthorization>,
    exhausted: bool,
    now: SystemTime,
) -> Option<String> {
    let OwnerReauthorization { previous, missing } = authority?;
    if !(exhausted || sandbox.work.retirement_credential_recovery.is_some())
        || !sandbox.allows_retirement_credential_recovery()
    {
        return None;
    }
    let newer_than_ms = if missing {
        let stopped_ms = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .unwrap_or(i64::MAX as u64);
        previous.granted_at_ms.max(stopped_ms)
    } else {
        previous.granted_at_ms
    };
    sandbox.work.retirement_credential_recovery =
        Some(layer2_core::sandbox::RetirementCredentialRecovery {
            newer_than_ms,
            expected_scope: previous.scope,
            admitted: None,
        });
    Some(rfc3339_at(
        now + layer2_core::sandbox::work_retry::WORK_RETRY_PROBE_INTERVAL,
    ))
}

impl SandboxWorker {
    pub(super) fn retirement_reauthorization_disabled(&self, sandbox: &Sandbox) -> bool {
        sandbox.work.retirement_credential_recovery.is_some()
            && !self
                .dependencies
                .credentials
                .uses_retirement_reauthorization(sandbox)
    }

    pub(super) async fn probe_retirement_reauthorization(
        &self,
        mut sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        use layer2_core::sandbox::{
            SandboxEventKind, SandboxStatus, SandboxWorkStep, TypedFailure,
        };

        let Some(mut recovery) = sandbox.work.retirement_credential_recovery else {
            return Err(SandboxStoreError::Conflict);
        };
        let observed = self
            .dependencies
            .credentials
            .retirement_authority_version(&sandbox)
            .await;
        let (ready_at, event) = match observed {
            Ok(Some(version)) if recovery.admits(version) => {
                // Commit admission before any provider effect. A replacement worker can
                // still dispatch the admitted generation and must verify it at that boundary.
                recovery.admitted = Some(version);
                tracing::info!(sandbox_id = %sandbox.id, "Retirement reauthorization verified");
                (
                    now,
                    SandboxEventKind::StatusChanged {
                        from: SandboxStatus::Retiring,
                        to: SandboxStatus::Retiring,
                    },
                )
            }
            Err(
                error @ (crate::sandbox::credential::CredentialError::Security(_)
                | crate::sandbox::credential::CredentialError::ServiceNotGranted(_)),
            ) => {
                return self
                    .retry_failure(sandbox, credential_failure(error), now)
                    .await;
            }
            _ => (
                now + layer2_core::sandbox::work_retry::WORK_RETRY_PROBE_INTERVAL,
                SandboxEventKind::WorkStepFailed {
                    step: SandboxWorkStep::RevokeRetirementCredentials,
                    failure: TypedFailure {
                        class: "owner_reauthorization_required".into(),
                        detail: "waiting for verified owner reauthorization".into(),
                    },
                },
            ),
        };
        sandbox.work.retirement_credential_recovery = Some(recovery);
        self.advance_at(
            sandbox,
            SandboxWorkStep::RevokeRetirementCredentials,
            now,
            ready_at,
            event,
        )
        .await
    }
}
