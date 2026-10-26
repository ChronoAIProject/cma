//! Independent CMAEG v1 management, custom event and agent tool clients.
mod channels;
mod outbound;
pub use outbound::OutboundSendError;
mod connections;
pub use connections::ConnectionOptions;
mod events;
mod signing;
mod tools;
pub mod types;
use crate::{Client, ClientBuilder, Result};
pub use events::{EventOptions, GatewayEvent, GatewayStream, TextDelta, TurnResult, WebhookAuth};
pub use signing::{WebhookHeaders, sign_webhook};
use std::time::Duration;

/// Gateway HTTP client, configured separately from the CMA deployment.
#[derive(Clone)]
pub struct Cmaeg {
    pub(crate) client: Client,
    test_timeout: Duration,
}

/// Gateway credentials and transport deadlines. Credentials are never printed.
pub struct CmaegBuilder(ClientBuilder, Duration);

impl Cmaeg {
    /// Configure a gateway origin without a `/v1` suffix.
    pub fn builder(base_url: impl AsRef<str>) -> Result<CmaegBuilder> {
        Ok(CmaegBuilder(
            Client::builder(base_url)?,
            Duration::from_secs(125),
        ))
    }
}

impl CmaegBuilder {
    /// Use a creator token for management, or the channel agent key for tools.
    pub fn bearer_token(self, token: impl AsRef<str>) -> Result<Self> {
        Ok(Self(self.0.bearer_token(token)?, self.1))
    }
    /// Deadline for the entire HTTP operation, including an event stream body.
    pub fn request_timeout(self, timeout: Duration) -> Result<Self> {
        Ok(Self(self.0.request_timeout(timeout)?, timeout))
    }
    /// Build without redirects or automatic request retries.
    pub fn build(self) -> Result<Cmaeg> {
        Ok(Cmaeg {
            client: self.0.build()?,
            test_timeout: self.1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deadline_does_not_change_other_requests_and_keeps_explicit_overrides() {
        let default = Cmaeg::builder("https://gateway.example")
            .unwrap()
            .bearer_token("fixture")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(
            default.client.inner.request_timeout,
            Duration::from_secs(30)
        );
        assert_eq!(default.test_timeout, Duration::from_secs(125));
        for seconds in [1, 300] {
            let timeout = Duration::from_secs(seconds);
            let explicit = Cmaeg::builder("https://gateway.example")
                .unwrap()
                .request_timeout(timeout)
                .unwrap()
                .bearer_token("fixture")
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(explicit.client.inner.request_timeout, timeout);
            assert_eq!(explicit.test_timeout, timeout);
        }
    }
}
