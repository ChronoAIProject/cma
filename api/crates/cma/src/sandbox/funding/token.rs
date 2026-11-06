//! Process-local producer credentials. Values deliberately have no Debug implementation.
use super::FundingError;
use async_trait::async_trait;
use std::sync::Arc;
use zeroize::Zeroizing;

pub(super) mod oauth;

#[derive(Clone)]
pub(super) struct ProducerToken(Arc<Zeroizing<String>>);
impl ProducerToken {
    pub(super) fn new(value: String) -> Self {
        Self(Arc::new(Zeroizing::new(value)))
    }
    pub(super) fn bearer(&self) -> &str {
        self.0.as_str()
    }
}

#[async_trait]
pub(super) trait ProducerTokenProvider: Send + Sync {
    async fn token(&self) -> Result<ProducerToken, FundingError>;
    /// None preserves static mode's original 401 behavior.
    async fn rejected(&self, _: &ProducerToken) -> Result<Option<ProducerToken>, FundingError> {
        Ok(None)
    }
}

pub(super) struct StaticTokenProvider(pub(super) String);
#[async_trait]
impl ProducerTokenProvider for StaticTokenProvider {
    async fn token(&self) -> Result<ProducerToken, FundingError> {
        std::env::var(&self.0).map(ProducerToken::new).map_err(|_| {
            FundingError::Invalid("producer credential reference is unavailable".into())
        })
    }
}
