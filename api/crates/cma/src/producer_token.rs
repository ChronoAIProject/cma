//! Process-local producer credentials. Values deliberately have no Debug implementation.

use async_trait::async_trait;
use std::sync::Arc;
use zeroize::Zeroizing;

pub(crate) mod oauth;

#[derive(Clone)]
pub(crate) struct ProducerToken(Arc<Zeroizing<String>>);
impl ProducerToken {
    pub(crate) fn new(value: String) -> Self {
        Self(Arc::new(Zeroizing::new(value)))
    }
    pub(crate) fn bearer(&self) -> &str {
        self.0.as_str()
    }
}

#[async_trait]
pub(crate) trait ProducerTokenProvider: Send + Sync {
    async fn token(&self) -> Result<ProducerToken, ProducerTokenError>;
    /// None preserves static mode's original 401 behavior.
    async fn rejected(
        &self,
        _: &ProducerToken,
    ) -> Result<Option<ProducerToken>, ProducerTokenError> {
        Ok(None)
    }
}

pub(crate) struct StaticTokenProvider(pub(crate) String);
#[async_trait]
impl ProducerTokenProvider for StaticTokenProvider {
    async fn token(&self) -> Result<ProducerToken, ProducerTokenError> {
        std::env::var(&self.0).map(ProducerToken::new).map_err(|_| {
            ProducerTokenError::Invalid("producer credential reference is unavailable".into())
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ProducerTokenError {
    #[error("{0}")]
    Retryable(String),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Clone, Copy)]
pub(crate) enum ProducerScope {
    Funding,
    Usage,
}
impl ProducerScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Funding => "billing:sandbox:write",
            Self::Usage => "billing:sandbox-usage:write billing:sandbox-usage:read",
        }
    }
}
