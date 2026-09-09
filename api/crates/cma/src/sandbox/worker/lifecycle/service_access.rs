use std::time::SystemTime;

use layer2_core::sandbox::store::{SandboxLifecycleAgentScope, SandboxStoreError};
use layer2_core::sandbox::{Sandbox, SandboxEventKind};
use layer2_core::{rfc3339_at, seconds_from_rfc3339};

use super::super::support::{credential_failure, WorkerFailure};
use super::super::{SandboxWorker, CREDENTIAL_EXPIRY_MARGIN};
use super::support::{lifecycle_lease, model_error};

impl SandboxWorker {
    pub(super) async fn renew_lifecycle_service_access(
        &self,
        sandbox: Sandbox,
        now: SystemTime,
    ) -> Result<(), SandboxStoreError> {
        let generation = sandbox
            .credential
            .next_generation()
            .ok_or_else(|| SandboxStoreError::Corrupt("credential generation exhausted".into()))?;
        let material = match self
            .dependencies
            .credentials
            .issue(&sandbox, generation, now)
            .await
        {
            Ok(material) => material,
            Err(error) => {
                return self
                    .handle_lifecycle_failure(sandbox, credential_failure(error), now)
                    .await;
            }
        };
        let minimum_expiry = (now + CREDENTIAL_EXPIRY_MARGIN)
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| SandboxStoreError::Conflict)?
            .as_secs();
        if material.sandbox_id != sandbox.id
            || material.owner != sandbox.owner
            || material.authority.generation != generation
            || seconds_from_rfc3339(&material.authority.expires_at)
                .is_none_or(|expiry| expiry <= minimum_expiry)
        {
            return self
                .handle_lifecycle_failure(
                    sandbox,
                    WorkerFailure::terminal_security("replacement service authority is invalid"),
                    now,
                )
                .await;
        }
        let at = rfc3339_at(now);
        let lease = lifecycle_lease(&sandbox, &self.config.holder)?;
        let mut next = sandbox.clone();
        next.replace_lifecycle_service_access_credential(
            material.authority,
            &lease.holder,
            lease.fence,
            at.clone(),
        )
        .map_err(model_error)?;
        self.commit_lifecycle_phase(
            &sandbox,
            next,
            Vec::new(),
            SandboxLifecycleAgentScope::Touched,
            at,
            vec![SandboxEventKind::CredentialChanged {
                key_generation: generation,
            }],
        )
        .await
    }
}
