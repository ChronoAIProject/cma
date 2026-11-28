//! Durable no-resend custody for a managed credential obligation.
use super::*;
use layer2_core::ServiceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IssuanceState {
    Unknown,
    Accepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum IssuanceIdentity {
    Prepared {
        provider_identity: String,
        request_digest: String,
    },
    /// An old worker may have dispatched without recording a request. Preserve
    /// that uncertainty rather than inventing a digest or a no-send proof.
    LegacyUnobserved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialIssuance {
    pub operation_id: String,
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub generation: u64,
    pub identity: IssuanceIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_authority: Option<layer2_core::sandbox::RetirementAuthorityVersion>,
    pub original_holder: String,
    pub original_fence: u64,
    pub created_at: String,
    pub next_check_at: String,
    pub state: IssuanceState,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum IssuanceObservation {
    MetadataOnly { matches: usize },
    Unavailable,
}

impl ManagedSandboxCredentials {
    pub(super) async fn issue_protected(
        &self,
        sandbox: &Sandbox,
        generation: u64,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        let original =
            sandbox.work.lease.as_ref().ok_or_else(|| {
                CredentialError::Security("credential work claim is absent".into())
            })?;
        let authorized = layer2_core::sandbox::store::WorkAuthority::current()
            .map_err(|_| CredentialError::Uncertain)?.iter().any(|authority| {
                matches!(authority.claimed(), layer2_core::sandbox::store::ClaimedWork::Sandbox(row)
                    if row.id == sandbox.id && row.owner == sandbox.owner
                    && row.work.lease.as_ref().is_some_and(|lease| lease.holder == original.holder && lease.fence == original.fence))
            });
        if !authorized {
            return Err(CredentialError::Uncertain);
        }
        let previous = self
            .custody
            .issuance(sandbox)
            .await
            .map_err(custody_error)?;
        if let Some(intent) = &previous {
            if intent.owner != sandbox.owner || intent.sandbox_id != sandbox.id {
                return Err(CredentialError::Security(
                    "credential obligation changed owner".into(),
                ));
            }
            if intent.state == IssuanceState::Unknown {
                return self.observe_unknown(sandbox, intent, now).await;
            }
            if intent.generation == generation {
                return self
                    .material(&sandbox.id, generation)
                    .await?
                    .ok_or(CredentialError::Uncertain)
                    .and_then(|material| owned_material(material, sandbox));
            }
            if intent.generation > sandbox.credential.key_generation {
                // A request/generation change cannot skip an accepted result
                // whose publication is still awaiting exact reconciliation.
                return Err(CredentialError::Uncertain);
            }
        }
        if sandbox.credential.next_generation() != Some(generation) {
            return Err(CredentialError::Security(
                "credential obligation is not current".into(),
            ));
        }
        if let Some(existing) = self.material(&sandbox.id, generation).await? {
            return owned_material(existing, sandbox);
        }
        if !sandbox.credential.source.is_managed() {
            return Err(CredentialError::Security(
                "provided credential is absent from custody".into(),
            ));
        }
        let operation_id = format!("mint_{}", uuid::Uuid::new_v4().simple());
        let token = self.owner_token(sandbox).await?;
        // All safe preparation failures occur before any durable intent. A
        // successor may repeat preparation, but cannot repeat a possible POST.
        let services = sandbox.credential.effective_reaches();
        let selection = sandbox.credential.selected_connection();
        let initial = sandbox.credential.selected_repository_connection();
        let requirement = initial.map(|connection| connection.requirement);
        let (prepared, github_route) = if let Some(selection) = selection {
            if selection.service.as_str() != crate::trigger::GITHUB_SERVICE {
                return Err(CredentialError::Security(
                    "unsupported service connection selection".into(),
                ));
            }
            let broker = self.exact_connections.as_ref().ok_or_else(|| {
                CredentialError::Unavailable("selected connection issuance is unavailable".into())
            })?;
            use crate::trigger::GitHubRepositoryNeed;
            use layer2_core::sandbox::RepositoryCredentialRequirement;
            let proof = match requirement {
                Some(requirement) => {
                    let need = match requirement {
                        RepositoryCredentialRequirement::PublicRepositoryWrite => {
                            GitHubRepositoryNeed::PublicRepositoryWrite
                        }
                        RepositoryCredentialRequirement::PrivateRepositoryRead => {
                            GitHubRepositoryNeed::PrivateRepositoryRead
                        }
                    };
                    broker
                        .verify_github_token_for(&token, &selection.connection_id, need)
                        .await
                }
                None => {
                    broker
                        .verify_github_repo_token(&token, &selection.connection_id)
                        .await
                }
            }
            .map_err(|error| match error {
                layer2_core::trigger_domain::ServiceConnectionError::Refused
                | layer2_core::trigger_domain::ServiceConnectionError::NotFound => {
                    CredentialError::ServiceNotGranted(selection.service.clone())
                }
                _ => CredentialError::Unavailable(
                    "selected GitHub connection could not be verified".into(),
                ),
            })?;
            let (prepared, mut route) = self
                .issuer
                .prepare_mint_with_connection(
                    &operation_id,
                    &sandbox.owner,
                    token.expose(),
                    &services,
                    (&ServiceId::new(crate::trigger::GITHUB_SERVICE), proof.id()),
                    |other_services| {
                        let route = layer3_adapter::GithubCredentialRoute {
                            service_id: proof.id().into(),
                            catalog_id: proof.catalog_id().into(),
                            repository_scope: None,
                            other_services: other_services
                                .into_iter()
                                .map(|service| layer3_adapter::PlatformServiceRoute {
                                    service_id: service.id,
                                    slug: service.slug,
                                    catalog_id: service.catalog_id,
                                })
                                .collect(),
                        };
                        route
                            .is_valid()
                            .then_some(route)
                            .ok_or(KeyIssuerError::UnexpectedAuthority)
                    },
                )
                .await
                .map_err(classify_issuer)?;
            if let (Some(requirement), Some(initial)) =
                (requirement, sandbox.credential.source.initial_connection())
            {
                route.repository_scope = Some(layer3_adapter::GithubRepositoryScope {
                    repository: initial.repository.clone(),
                    required_scope: match requirement {
                        RepositoryCredentialRequirement::PublicRepositoryWrite => {
                            layer3_adapter::GithubRepositoryScopeNeed::PublicRepo
                        }
                        RepositoryCredentialRequirement::PrivateRepositoryRead => {
                            layer3_adapter::GithubRepositoryScopeNeed::Repo
                        }
                    },
                    manage_url: proof.manage_url,
                });
                if !route.is_valid() {
                    return Err(CredentialError::Security(
                        "GitHub repository scope authority is invalid".into(),
                    ));
                }
            }
            (prepared, Some(route))
        } else {
            let prepared = self
                .issuer
                .prepare_mint(&operation_id, &sandbox.owner, token.expose(), &services)
                .await
                .map_err(classify_issuer)?;
            (prepared, None)
        };
        let owner_authority = self.issuance_owner_authority(sandbox, &token).await;
        let intent = CredentialIssuance {
            operation_id,
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            generation,
            identity: IssuanceIdentity::Prepared {
                provider_identity: prepared.provider_identity().into(),
                request_digest: prepared.request_digest().into(),
            },
            owner_authority,
            original_holder: original.holder.clone(),
            original_fence: original.fence,
            created_at: rfc3339_at(now),
            next_check_at: rfc3339_at(now + std::time::Duration::from_secs(30)),
            state: IssuanceState::Unknown,
        };
        if !self
            .custody
            .begin_issuance(sandbox, &intent)
            .await
            .map_err(custody_error)?
        {
            return Err(CredentialError::Uncertain);
        }
        // An uncertain intent acknowledgement intentionally never reaches this
        // consuming call. Losing availability here preserves no-resend custody.
        let minted = prepared.dispatch().await.map_err(|failure| {
            tracing::warn!(sandbox_id = %sandbox.id, generation,
                phase = failure.phase, http_status = failure.http_status,
                "credential dispatch outcome is uncertain; automatic mint retry refused");
            CredentialError::Uncertain
        })?;
        let material = CredentialMaterial {
            source: sandbox.credential.source.clone(),
            github_route,
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            authority: SandboxCredentialAuthority {
                generation,
                provider_key_id: minted.id,
                provider_state_version: minted.state_version,
                issued_at: intent.created_at.clone(),
                expires_at: rfc3339_at(minted.expires_at),
                grant_fingerprint: minted.grant_fingerprint,
            },
            value: minted.value,
        };
        match self
            .custody
            .accept_issuance(sandbox, &intent, &material)
            .await
        {
            Ok(()) => Ok(material),
            Err(error) => {
                tracing::warn!(sandbox_id = %sandbox.id, error = %error,
                    "credential result persistence requires exact reconciliation");
                // This is read-only exact-result reconciliation. It never
                // invokes mint or revokes a possibly committed accepted result.
                let stored = self.material(&sandbox.id, generation).await?;
                match stored {
                    Some(stored)
                        if stored.owner == material.owner
                            && stored.source == material.source
                            && stored.authority == material.authority
                            && stored.github_route == material.github_route
                            && stored.value.expose() == material.value.expose() =>
                    {
                        Ok(stored)
                    }
                    _ => Err(CredentialError::Uncertain),
                }
            }
        }
    }

    async fn observe_unknown(
        &self,
        sandbox: &Sandbox,
        intent: &CredentialIssuance,
        now: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        if intent.next_check_at > rfc3339_at(now) {
            return Err(CredentialError::Uncertain);
        }
        let identity_matches = match &intent.identity {
            IssuanceIdentity::Prepared {
                provider_identity, ..
            } => provider_identity == &self.issuer.provider_identity(),
            IssuanceIdentity::LegacyUnobserved => true,
        };
        let provider_key_name = match intent.identity {
            IssuanceIdentity::Prepared { .. } => intent.operation_id.as_str(),
            IssuanceIdentity::LegacyUnobserved => intent.sandbox_id.as_str(),
        };
        let observation = match (identity_matches, self.owner_token(sandbox).await) {
            (true, Ok(token)) => match self.issuer.owned_keys(token.expose()).await {
                Ok(keys) => IssuanceObservation::MetadataOnly {
                    matches: keys
                        .iter()
                        .filter(|key| key.credential_id == provider_key_name)
                        .count(),
                },
                Err(_) => IssuanceObservation::Unavailable,
            },
            // Missing current credentials or a changed endpoint is not proof
            // that the original effect failed. Keep bounded reconciliation due.
            _ => IssuanceObservation::Unavailable,
        };
        self.custody
            .observe_issuance(sandbox, intent, observation, now)
            .await
            .map_err(custody_error)?;
        tracing::debug!(sandbox_id = %sandbox.id, generation = intent.generation,
            "credential observation retained unresolved issuance for reconciliation");
        // Listing has no secret replay or operation finality contract. Neither
        // an empty list nor a matching key authorizes a second credential POST.
        Err(CredentialError::Uncertain)
    }
}
