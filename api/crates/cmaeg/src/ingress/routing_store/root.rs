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
        profile: Option<&crate::channels::profile_assignment::Admission>,
    ) -> Result<(), RecordError> {
        if let Some(profile) = profile {
            let source_guards = match guard.as_ref() {
                Some((id, version, guards))
                    if id == &profile.channel_record_id && *version == profile.channel_version =>
                {
                    guards.as_slice()
                }
                Some(_) => {
                    return Err(RecordError::GuardConflict {
                        scope: "channel",
                        expected: profile.channel_version,
                    })
                }
                None => &[],
            };
            let blob = seal_interaction(&self.keys, &manifest.id, bytes)
                .map_err(|_| RecordError::Rejected("routed root encryption failed".into()))?;
            let projection = self
                .admission_queue
                .as_ref()
                .and_then(|queue| queue.index_deployment())
                .map(|deployment| crate::records::mongo::ReplicaProjection {
                    kind: crate::records::mongo::ReplicaKind::Admission,
                    deployment: deployment.into(),
                    active: true,
                });
            self.records
                .put_profile_admission(
                    &manifest.id,
                    &manifest.identity.owner,
                    &blob,
                    manifest.retained_until,
                    profile,
                    source_guards,
                    projection,
                )
                .await?;
            return Ok(());
        }
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
