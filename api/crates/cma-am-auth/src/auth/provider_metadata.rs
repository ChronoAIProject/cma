//! Provider discovery shared by authentication paths.

use super::{
    rebase, verification_error::ProviderVerificationError as Error, AuthMode, AuthState,
    ProviderMetadata,
};

impl AuthState {
    pub async fn metadata(&self) -> Result<ProviderMetadata, String> {
        self.verification_metadata()
            .await
            .map_err(|error| error.to_string())
    }

    pub(super) async fn verification_metadata(&self) -> Result<ProviderMetadata, Error> {
        self.verification_metadata_until(
            tokio::time::Instant::now() + super::identity_http::DIRECT_TIMEOUT,
        )
        .await
    }

    pub(super) async fn verification_metadata_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<ProviderMetadata, Error> {
        if let Some(metadata) = self.metadata.read().await.clone() {
            return Ok(metadata);
        }
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return super::verification_diagnostics::configuration(
                super::verification_diagnostics::Endpoint::Discovery,
                Err(Error::Unavailable(
                    "no identity provider is configured".into(),
                )),
            );
        };
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            base_url.trim_end_matches('/')
        );
        let body = super::identity_http::json(
            self.http.get(&discovery_url),
            deadline,
            super::verification_diagnostics::Endpoint::Discovery,
        )
        .await?;
        let parsed = (|| {
            let mut metadata: ProviderMetadata = serde_json::from_value(body)
                .map_err(|_| Error::Unavailable("invalid identity provider metadata".into()))?;
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
            Ok(metadata)
        })();
        let metadata = super::verification_diagnostics::claims(
            super::verification_diagnostics::Endpoint::Discovery,
            parsed,
        )?;
        *self.metadata.write().await = Some(metadata.clone());
        Ok(metadata)
    }
}
