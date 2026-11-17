//! Durable no-resend custody for a managed credential obligation.
use super::*;
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
        let prepared = self
            .issuer
            .prepare_mint(
                &operation_id,
                &sandbox.owner,
                token.expose(),
                &sandbox.credential.effective_reaches(),
            )
            .await
            .map_err(classify_issuer)?;
        let intent = CredentialIssuance {
            operation_id,
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            generation,
            identity: IssuanceIdentity::Prepared {
                provider_identity: prepared.provider_identity().into(),
                request_digest: prepared.request_digest().into(),
            },
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
        let minted = prepared.dispatch().await.map_err(|_| {
            tracing::warn!(sandbox_id = %sandbox.id, generation,
                "credential dispatch outcome is uncertain; automatic mint retry refused");
            CredentialError::Uncertain
        })?;
        let material = CredentialMaterial {
            source: Default::default(),
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
