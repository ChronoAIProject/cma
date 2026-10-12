//! Independent CMAEG v1 management, custom event and agent tool clients.
mod channels;
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
}

/// Gateway credentials and transport deadlines. Credentials are never printed.
pub struct CmaegBuilder(ClientBuilder);

impl Cmaeg {
    /// Configure a gateway origin without a `/v1` suffix.
    pub fn builder(base_url: impl AsRef<str>) -> Result<CmaegBuilder> {
        Ok(CmaegBuilder(Client::builder(base_url)?))
    }
}

impl CmaegBuilder {
    /// Use a creator token for management, or the channel agent key for tools.
    pub fn bearer_token(self, token: impl AsRef<str>) -> Result<Self> {
        Ok(Self(self.0.bearer_token(token)?))
    }
    /// Deadline for the entire HTTP operation, including an event stream body.
    pub fn request_timeout(self, timeout: Duration) -> Result<Self> {
        Ok(Self(self.0.request_timeout(timeout)?))
    }
    /// Build without redirects or automatic request retries.
    pub fn build(self) -> Result<Cmaeg> {
        Ok(Cmaeg {
            client: self.0.build()?,
        })
    }
}
