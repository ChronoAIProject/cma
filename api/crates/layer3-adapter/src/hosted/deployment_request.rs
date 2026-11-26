//! New work renders the release it was admitted against.
use super::{
    deployment::{RUNTIME_GENERATION_METADATA, RUNTIME_IMAGE_METADATA, SPEC_FINGERPRINT_METADATA},
    image_sha256, provider_request_fingerprint, provider_request_identity, validate_runtime_image,
    HostedDeployment,
};
use crate::{provider::ProviderSandboxRequest, AdapterError, RuntimeProvisionRequest, SandboxSpec};
use std::borrow::Cow;

pub(crate) const JOURNAL_RECOVERY: &str = "CHRONO_RUNTIME_LEGACY_JOURNAL_RECOVERY_ENABLED";
const RUNTIME_GENERATION: &str = "CHRONO_RUNTIME_GENERATION";

impl RuntimeProvisionRequest {
    pub(in crate::hosted) fn released_spec(&self) -> Cow<'_, SandboxSpec> {
        match (&self.spec.runtime_image, &self.released) {
            (None, Some(released)) => {
                let mut spec = self.spec.clone();
                spec.runtime_image = Some(released.agent.clone());
                Cow::Owned(spec)
            }
            _ => Cow::Borrowed(&self.spec),
        }
    }
}

impl HostedDeployment {
    /// A recorded image wins over the release captured for new work.
    pub fn render_request(
        &self,
        request: &RuntimeProvisionRequest,
    ) -> crate::Result<ProviderSandboxRequest> {
        self.render_runtime(&request.released_spec(), request.generation)
    }
}

impl ProviderSandboxRequest {
    /// Render an admitted retained successor from the frozen predecessor.
    /// The caller supplies the exact reserved generation and release image.
    pub fn retained_startup_policy_successor(
        &self,
        image: &str,
        generation: u64,
        journal_recovery: bool,
    ) -> crate::Result<Self> {
        validate_runtime_image(&self.image.uri)?;
        validate_runtime_image(image)?;
        let predecessor = provider_request_identity(self)?;
        if generation <= predecessor.generation
            || self.volumes.is_empty()
            || self
                .metadata
                .get("cma/home-identity")
                .is_none_or(|identity| !crate::valid_home_identity(identity))
            || self.env.get(RUNTIME_GENERATION) != self.metadata.get(RUNTIME_GENERATION_METADATA)
        {
            return Err(AdapterError::Rejected(
                "retained startup policy requires an exact home and a reserved successor generation"
                    .into(),
            ));
        }
        let mut successor = self.clone();
        successor.metadata.remove(SPEC_FINGERPRINT_METADATA);
        if provider_request_fingerprint(&successor)? != predecessor.spec_fingerprint {
            return Err(AdapterError::Rejected(
                "frozen retained runtime fingerprint does not match".into(),
            ));
        }

        successor.image.uri = image.to_owned();
        let generation = generation.to_string();
        successor
            .metadata
            .insert(RUNTIME_GENERATION_METADATA.into(), generation.clone());
        successor
            .metadata
            .insert(RUNTIME_IMAGE_METADATA.into(), image_sha256(image));
        successor.env.insert(RUNTIME_GENERATION.into(), generation);
        successor
            .env
            .insert(JOURNAL_RECOVERY.into(), journal_recovery.to_string());
        successor.metadata.insert(
            SPEC_FINGERPRINT_METADATA.into(),
            provider_request_fingerprint(&successor)?,
        );
        Ok(successor)
    }
}

pub(super) fn apply_retained_journal_policy(
    spec: &crate::SandboxSpec,
    env: &mut std::collections::BTreeMap<String, String>,
) -> crate::Result<()> {
    if let Some(journal_recovery) = spec.retained_journal_recovery {
        if spec.home_volume.as_ref().is_none_or(|home| {
            !home.request.policy.is_native_vct() || home.receipt.native_authority.is_none()
        }) {
            return Err(crate::AdapterError::Rejected(
                "retained startup policy requires an original home attachment".into(),
            ));
        }
        env.insert(JOURNAL_RECOVERY.into(), journal_recovery.to_string());
    }
    Ok(())
}
