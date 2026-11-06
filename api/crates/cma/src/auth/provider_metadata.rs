//! Provider discovery shared by authentication paths.

use super::{
    identity, rebase, verification_error::ProviderVerificationError as Error, AuthMode, AuthState,
    ProviderMetadata,
};

impl AuthState {
    pub(super) async fn metadata(&self) -> Result<ProviderMetadata, String> {
        self.verification_metadata()
            .await
            .map_err(|error| error.to_string())
    }

    pub(super) async fn verification_metadata(&self) -> Result<ProviderMetadata, Error> {
        if let Some(metadata) = self.metadata.read().await.clone() {
            return Ok(metadata);
        }
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(Error::Unavailable(
                "no identity provider is configured".into(),
            ));
        };
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            base_url.trim_end_matches('/')
        );
        let response = self
            .http
            .get(&discovery_url)
            .send()
            .await
            .map_err(|error| {
                identity::verification_failure("discovery", "transport", None);
                Error::transport(error)
            })?;
        if !response.status().is_success() {
            identity::verification_failure(
                "discovery",
                "http_status",
                Some(response.status().as_u16()),
            );
            return Err(Error::status(response.status()));
        }
        let mut metadata = response
            .json::<ProviderMetadata>()
            .await
            .map_err(Error::transport)?;
        metadata.token_endpoint =
            rebase(&metadata.token_endpoint, base_url).map_err(Error::Unavailable)?;
        metadata.userinfo_endpoint = metadata
            .userinfo_endpoint
            .as_deref()
            .map(|endpoint| rebase(endpoint, base_url))
            .transpose()
            .map_err(Error::Unavailable)?;
        metadata.introspection_endpoint = metadata
            .introspection_endpoint
            .as_deref()
            .map(|endpoint| rebase(endpoint, base_url))
            .transpose()
            .map_err(Error::Unavailable)?;
        *self.metadata.write().await = Some(metadata.clone());
        Ok(metadata)
    }
}
