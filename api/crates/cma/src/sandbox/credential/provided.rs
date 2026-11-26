use super::*;
use layer2_core::sandbox::binding::{BindingCredentialCustody, ProvidedCredential};
use layer2_core::sandbox::SandboxCredentialSource;
use layer2_core::{CoreError, ServiceId};

#[async_trait::async_trait]
impl BindingCredentialCustody for ManagedSandboxCredentials {
    async fn retain(
        &self,
        sandbox_id: &SandboxId,
        owner: &SubjectId,
        services: &[ServiceId],
        credential: &ProvidedCredential,
    ) -> Result<(), CoreError> {
        let material = Self::provided_material(sandbox_id, owner, services, credential)?;
        match self.custody.insert(material.clone()).await {
            Ok(()) => Ok(()),
            Err(CredentialCustodyError::Duplicate) => {
                let existing = self
                    .custody
                    .get(sandbox_id, 1)
                    .await
                    .map_err(failed)?
                    .ok_or_else(|| {
                        CoreError::RuntimeUnavailable(
                            "provided credential custody disappeared".into(),
                        )
                    })?;
                if existing.sandbox_id != *sandbox_id
                    || existing.owner != *owner
                    || existing.source != material.source
                    || existing.authority.grant_fingerprint != material.authority.grant_fingerprint
                    || existing.value.expose() != material.value.expose()
                {
                    return Err(CoreError::InvalidState(
                        "provided credential custody conflicts with this binding".into(),
                    ));
                }
                Ok(())
            }
            Err(error) => Err(failed(error)),
        }
    }
}

impl ManagedSandboxCredentials {
    pub(crate) fn binding_material(
        binding: &layer2_core::sandbox::binding::ProviderBinding,
        credential: &ProvidedCredential,
    ) -> Result<CredentialMaterial, CoreError> {
        let services = &binding
            .profile
            .environment
            .as_ref()
            .ok_or_else(|| CoreError::InvalidState("missing binding environment".into()))?
            .sandbox
            .nyxid_services;
        let mut material =
            Self::provided_material(&binding.sandbox_id, &binding.owner, services, credential)?;
        material.source = binding.credential_source();
        Ok(material)
    }

    pub(crate) fn provided_material(
        sandbox_id: &SandboxId,
        owner: &SubjectId,
        services: &[ServiceId],
        credential: &ProvidedCredential,
    ) -> Result<CredentialMaterial, CoreError> {
        let supplied = KeyIssuer::provided(credential.expose())
            .map_err(|_| CoreError::InvalidRequest("invalid supplied agent key".into()))?;
        let hash = credential.sha256();
        let mut services = services.to_vec();
        services.sort();
        services.dedup();
        let material = CredentialMaterial {
            github_route: None,
            source: SandboxCredentialSource::Provided {
                key_sha256: hash.clone(),
            },
            sandbox_id: sandbox_id.clone(),
            owner: owner.clone(),
            authority: SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: format!("provided-{hash}"),
                provider_state_version: 1,
                issued_at: layer2_core::now_rfc3339(),
                expires_at: String::new(),
                grant_fingerprint: layer2_core::profiles::content_digest(&services).map_err(
                    |_| CoreError::InvalidRequest("invalid credential service scope".into()),
                )?,
            },
            value: supplied,
        };
        Ok(material)
    }
}

fn failed(_: CredentialCustodyError) -> CoreError {
    CoreError::RuntimeUnavailable("provided credential custody is unavailable".into())
}
