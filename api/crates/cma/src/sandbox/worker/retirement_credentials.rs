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
                Err(error) => {
                    failure.get_or_insert_with(|| credential_failure(error));
                }
            }
        }
        sandbox.credential.current = None;
        sandbox.credential.pending = None;
        sandbox.credential.delivered = false;
        if let Some(failure) = failure {
            return Err(failure);
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
