//! Anonymous diagnostic categories; never send exception text or identifiers.
use crate::{Client, Result, http::Request};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// A fixed, content-free browser diagnostic classification.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFault {
    /// An uncaught script exception.
    UncaughtError,
    /// A promise rejection without a handler.
    UnhandledRejection,
    /// A React render failure.
    ReactError,
    /// A script or stylesheet failed to load.
    ResourceError,
    /// A lazy application chunk failed to load.
    ChunkError,
}
impl Client {
    /// Send one untrusted diagnostic anonymously; no durable receipt or retry.
    pub async fn report_browser_fault(&self, kind: BrowserFault) -> Result<()> {
        #[derive(Serialize)]
        struct Report {
            kind: BrowserFault,
        }
        let mut request = Request::operation(
            Method::POST,
            &["telemetry", "browser"],
            Some(&Report { kind }),
            StatusCode::NO_CONTENT,
        )?;
        request.anonymous = true;
        self.empty(request).await
    }
}
