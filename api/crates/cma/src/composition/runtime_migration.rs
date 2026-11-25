//! File publication adapter for server-owned runtime migration authority.
use layer2_core::sandbox::store::{RuntimeMigrationClearance, SandboxStoreError};
use layer2_core::sandbox::{
    RuntimeActivityVerifier, RuntimeMigrationAuthority, RuntimeMigrationAuthorityEntry,
    RuntimeMigrationBundle, RuntimeMigrationPolicy, MAX_RUNTIME_MIGRATION_BUNDLE_BYTES,
};
use layer2_core::SandboxId;
use std::{path::PathBuf, sync::Arc};
use tokio::io::AsyncReadExt;

pub(super) struct FileRuntimeMigrationAuthority {
    policy: RuntimeMigrationPolicy,
    bundle_path: PathBuf,
    verifier: Arc<RuntimeActivityVerifier>,
}
impl FileRuntimeMigrationAuthority {
    pub(super) fn new(
        policy: RuntimeMigrationPolicy,
        bundle_path: PathBuf,
        verifier: Arc<RuntimeActivityVerifier>,
    ) -> Self {
        Self {
            policy,
            bundle_path,
            verifier,
        }
    }
}
#[async_trait::async_trait]
impl RuntimeMigrationAuthority for FileRuntimeMigrationAuthority {
    fn policy(&self) -> &RuntimeMigrationPolicy {
        &self.policy
    }
    async fn resolve(
        &self,
        id: &SandboxId,
    ) -> Result<RuntimeMigrationAuthorityEntry, SandboxStoreError> {
        let file = tokio::fs::File::open(&self.bundle_path)
            .await
            .map_err(|_| {
                SandboxStoreError::Unavailable("runtime migration bundle unavailable".into())
            })?;
        let mut bytes = Vec::new();
        file.take((MAX_RUNTIME_MIGRATION_BUNDLE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| {
                SandboxStoreError::Unavailable("runtime migration bundle unreadable".into())
            })?;
        let bundle = RuntimeMigrationBundle::parse(&bytes, &self.policy)?;
        let mut entry = bundle
            .entries
            .into_iter()
            .find(|entry| &entry.identity.sandbox_id == id)
            .ok_or(SandboxStoreError::Conflict)?;
        if let Some(reference) = entry.observation_reference.take() {
            entry.clearance = RuntimeMigrationClearance::Observed {
                proof: Box::new(self.verifier.proof_for(id, &reference).await?),
            };
        }
        self.policy.validate_entry(
            &entry,
            layer2_core::sandbox::store::runtime_migration_now_ms()?,
        )?;
        Ok(entry)
    }
}
