//! Root insertion composes source authority and optional replica discovery.
use super::{types::Manifest, PlanStore};
use crate::records::{AdmissionRootWrite, AuthorityGuard, RecordError};
use cmaeg_core::seal::seal_interaction;

type SourceGuard = Option<(String, u64, Vec<AuthorityGuard>)>;
impl PlanStore {
    pub(super) async fn create_root(
        &self,
        manifest: &Manifest,
        bytes: &[u8],
        guard: SourceGuard,
    ) -> Result<(), RecordError> {
        if let Some(deployment) = self
            .admission_queue
            .as_ref()
            .and_then(|queue| queue.index_deployment())
        {
            let blob = seal_interaction(&self.keys, &manifest.id, bytes)
                .map_err(|_| RecordError::Rejected("routed root encryption failed".into()))?;
            let version = self
                .records
                .put_admission_root(AdmissionRootWrite {
                    id: &manifest.id,
                    owner: &manifest.identity.owner,
                    blob: &blob,
                    expires_at: manifest.retained_until,
                    deployment,
                    github_guard: guard
                        .as_ref()
                        .map(|(id, version, guards)| (id.as_str(), *version, guards.as_slice())),
                })
                .await?;
            return if version == 1 {
                Ok(())
            } else {
                Err(RecordError::Malformed("invalid root create version".into()))
            };
        }
        match guard {
            Some((id, version, guards)) => {
                self.create_guarded(&manifest.id, manifest, bytes, &id, version, &guards)
                    .await
            }
            None => self.create(&manifest.id, manifest, bytes).await,
        }
    }
}
