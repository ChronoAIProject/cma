//! Separate retryable grant preparation from the consuming credential POST.
use super::*;

/// No Clone implementation: only the originating preparation can consume this
/// request. A persisted intent never reconstructs a dispatch capability.
pub(crate) struct PreparedMint {
    request: reqwest::RequestBuilder,
    allowed_services: Vec<String>,
    allowed_nodes: Vec<String>,
    expires_at: SystemTime,
    request_digest: String,
    provider_identity: String,
}

impl PreparedMint {
    pub(crate) fn request_digest(&self) -> &str {
        &self.request_digest
    }
    pub(crate) fn provider_identity(&self) -> &str {
        &self.provider_identity
    }

    pub(crate) async fn dispatch(self) -> Result<MintedKey, KeyIssuerError> {
        // Renewal owns the local window; preparation and intent persistence
        // cannot reserve a fresh window or recover an expired permit.
        let authorities = layer2_core::sandbox::store::WorkAuthority::current()
            .map_err(|_| KeyIssuerError::UnexpectedAuthority)?;
        let deadline = authorities
            .iter()
            .map(|authority| authority.deadline())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| KeyIssuerError::UnexpectedAuthority)?
            .into_iter()
            .min();
        let request = match deadline {
            Some(deadline) => self
                .request
                .timeout(deadline.saturating_duration_since(tokio::time::Instant::now())),
            None => self.request,
        };
        let response = request
            .send()
            .await
            .map_err(|error| KeyIssuerError::Unavailable(error.to_string()))?;
        if !response.status().is_success() {
            return Err(KeyIssuerError::Refused(response.status().as_u16()));
        }
        let response = response
            .json::<MintedKeyResponse>()
            .await
            .map_err(|error| KeyIssuerError::Unavailable(error.to_string()))?;
        minted_key_from_response(
            response,
            &self.allowed_services,
            &self.allowed_nodes,
            self.expires_at,
        )
        .ok_or(KeyIssuerError::UnexpectedAuthority)
    }
}

impl KeyIssuer {
    pub(crate) fn provider_identity(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(format!("{}\0{}", self.base_url, self.deployment))
        )
    }

    pub(crate) async fn prepare_mint(
        &self,
        credential_id: &str,
        owner: &layer2_core::SubjectId,
        owner_token: &str,
        services: &[ServiceId],
    ) -> Result<PreparedMint, KeyIssuerError> {
        validate_credential_id(credential_id)?;
        let allowed = self.resolve_exact(owner_token, services).await?;
        self.prepare_allowed_mint(credential_id, owner, owner_token, allowed)
            .await
    }

    pub(crate) async fn prepare_github_mint(
        &self,
        credential_id: &str,
        owner: &layer2_core::SubjectId,
        owner_token: &str,
        services: &[ServiceId],
        connection: &crate::trigger::VerifiedGitHubConnection,
    ) -> Result<(PreparedMint, layer3_adapter::GithubCredentialRoute), KeyIssuerError> {
        use crate::trigger::GITHUB_SERVICE;
        if !services
            .iter()
            .any(|service| service.as_str() == GITHUB_SERVICE)
        {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let others = services
            .iter()
            .filter(|service| service.as_str() != GITHUB_SERVICE)
            .cloned()
            .collect::<Vec<_>>();
        let mut route = layer3_adapter::GithubCredentialRoute {
            service_id: connection.id().into(),
            catalog_id: connection.catalog_id().into(),
            repository_scope: None,
            other_services: Vec::new(),
        };
        if !others.is_empty() {
            let rows = self.service_rows(USER_SERVICES_PATH, owner_token).await?;
            for row in Self::resolve_service_rows(&rows, &others)? {
                if row.catalog_service_id.as_deref() == Some(connection.catalog_id()) {
                    return Err(KeyIssuerError::UnexpectedAuthority);
                }
                route
                    .other_services
                    .push(layer3_adapter::PlatformServiceRoute {
                        service_id: row.id.clone(),
                        slug: row
                            .slug
                            .clone()
                            .ok_or(KeyIssuerError::UnexpectedAuthority)?,
                        catalog_id: row.catalog_service_id.clone(),
                    });
            }
        }
        if !route.is_valid() {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let mut allowed = route
            .other_services
            .iter()
            .map(|service| service.service_id.clone())
            .collect::<Vec<_>>();
        allowed.push(connection.id().to_owned());
        let prepared = self
            .prepare_allowed_mint(credential_id, owner, owner_token, allowed)
            .await?;
        Ok((prepared, route))
    }

    async fn prepare_allowed_mint(
        &self,
        credential_id: &str,
        owner: &layer2_core::SubjectId,
        owner_token: &str,
        allowed: Vec<String>,
    ) -> Result<PreparedMint, KeyIssuerError> {
        validate_credential_id(credential_id)?;
        let plan = self
            .scope_plan(
                owner_token,
                &allowed,
                &layer2_core::resource_access::ResourceOwner::Person {
                    subject: owner.clone(),
                },
            )
            .await?;
        let expires_at = key_expiry(SystemTime::now());
        let mut body = mint_request_body(&self.deployment, credential_id, &allowed, expires_at)?;
        plan.apply(&mut body);
        let encoded = serde_json::to_vec(&body).map_err(|_| KeyIssuerError::UnexpectedAuthority)?;
        Ok(PreparedMint {
            request: self
                .http
                .post(format!("{}{KEYS_PATH}", self.base_url))
                .bearer_auth(owner_token)
                .json(&body),
            allowed_services: allowed,
            allowed_nodes: plan.allowed_node_ids,
            expires_at,
            request_digest: format!("{:x}", Sha256::digest(encoded)),
            provider_identity: format!(
                "{:x}",
                Sha256::digest(format!("{}\0{}", self.base_url, self.deployment))
            ),
        })
    }
}
