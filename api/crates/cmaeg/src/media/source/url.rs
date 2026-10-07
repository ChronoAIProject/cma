//! Custom attachments named by HTTPS URL, from allowlisted origins only.
//!
//! The URL comes from a custom sender, so every fetch is held to the
//! channel's *current* exact-origin allowlist and to the gateway's SSRF policy:
//! every DNS answer must be public, and the connection is pinned to the
//! checked addresses, with no proxy, redirect or cookie.
use super::{FetchRequest, MediaSource, SourceStream};
use crate::{
    channels::resolver::ResolvedChannel,
    media::{
        body::{read_capped, stream_capped, transport, FetchedBody},
        FetchError,
    },
    net::resolve::{all_public, pinned_client, resolve},
};
use cmaeg_core::{
    activity::{AttachmentSource, SourceKind},
    channel::Source,
};
use futures_util::future::BoxFuture;
use reqwest::{
    header::{ACCEPT, USER_AGENT},
    StatusCode, Url,
};
use std::{sync::Arc, time::Duration};
use tokio::time::{timeout_at, Instant};

const URL_DEADLINE: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const AGENT: &str = "cmaeg-media/1";

/// Sends the GET for an already authorized URL.
///
/// The network policy lives in [`PinnedTransport`]; the seam exists so tests
/// can serve bytes from loopback, which the policy rightly refuses.
pub trait UrlTransport: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<reqwest::Response, FetchError>>;
}

/// The production transport: public addresses only, pinned per fetch.
pub struct PinnedTransport;

impl UrlTransport for PinnedTransport {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<reqwest::Response, FetchError>> {
        Box::pin(async move {
            let host = url.host_str().ok_or(FetchError::Rejected)?;
            let addresses = timeout_at(deadline, resolve(url))
                .await
                .map_err(|_| FetchError::TimedOut)?
                .map_err(|_| FetchError::Upstream)?;
            if !all_public(&addresses) {
                tracing::info!(
                    addresses = addresses.len(),
                    "custom attachment origin resolved to a non-public address; refused"
                );
                return Err(FetchError::Rejected);
            }
            let total = deadline.saturating_duration_since(Instant::now());
            let client =
                pinned_client(host, &addresses, CONNECT_TIMEOUT, total).map_err(transport)?;
            let request = client
                .get(url.clone())
                .header(ACCEPT, "*/*")
                .header(USER_AGENT, AGENT);
            match timeout_at(deadline, request.send()).await {
                Err(_) => Err(FetchError::TimedOut),
                Ok(Err(error)) => Err(transport(error)),
                Ok(Ok(response)) => Ok(response),
            }
        })
    }
}

/// Fetches `url{url}` locators.
pub struct UrlSource {
    transport: Arc<dyn UrlTransport>,
}

impl UrlSource {
    pub fn new(transport: Arc<dyn UrlTransport>) -> Self {
        Self { transport }
    }

    /// The production source, behind [`PinnedTransport`].
    pub fn pinned() -> Self {
        Self::new(Arc::new(PinnedTransport))
    }

    async fn response(
        &self,
        request: &FetchRequest<'_>,
    ) -> Result<(reqwest::Response, Instant), FetchError> {
        let (Some(AttachmentSource::Url { url }), SourceKind::Custom) =
            (request.locator, &request.activity.source)
        else {
            return Err(FetchError::NotRetrievable);
        };
        let url = Url::parse(url).map_err(|_| FetchError::Rejected)?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(FetchError::Rejected);
        }
        // The allowlist is re-read from the current definition, so an origin
        // removed since the event arrived is no longer fetched.
        if !allowed(request.channel, &url.origin().ascii_serialization()) {
            tracing::info!("custom attachment origin is not in the channel's current allowlist");
            return Err(FetchError::Rejected);
        }
        let deadline = request.deadline.min(Instant::now() + URL_DEADLINE);
        let response = self.transport.get(&url, deadline).await?;
        classify(response.status())?;
        Ok((response, deadline))
    }
}

fn allowed(channel: &ResolvedChannel, origin: &str) -> bool {
    channel.definition.sources.iter().any(|source| {
        matches!(
            source,
            Source::Custom { attachments: Some(attachments), .. }
                if attachments.origins.iter().any(|allowed| allowed == origin)
        )
    })
}

/// Redirects are never followed, so a 3xx is a refusal.
fn classify(status: StatusCode) -> Result<(), FetchError> {
    match status.as_u16() {
        200..=299 => Ok(()),
        404 => Err(FetchError::NotFound),
        429 => Err(FetchError::RateLimited),
        503 => Err(FetchError::Busy),
        500..=599 => Err(FetchError::Upstream),
        _ => Err(FetchError::Rejected),
    }
}

impl MediaSource for UrlSource {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        Box::pin(async move {
            let (response, deadline) = self.response(&request).await?;
            read_capped(response, request.cap, request.overflow, deadline).await
        })
    }

    fn open<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async move {
            let (response, _) = self.response(&request).await?;
            stream_capped(response, request.cap)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_fixed_classifications() {
        for (status, expected) in [
            (200, Ok(())),
            (206, Ok(())),
            (301, Err(FetchError::Rejected)),
            (302, Err(FetchError::Rejected)),
            (307, Err(FetchError::Rejected)),
            (404, Err(FetchError::NotFound)),
            (429, Err(FetchError::RateLimited)),
            (403, Err(FetchError::Rejected)),
            (503, Err(FetchError::Busy)),
            (500, Err(FetchError::Upstream)),
        ] {
            assert_eq!(
                classify(StatusCode::from_u16(status).unwrap()),
                expected,
                "{status}"
            );
        }
    }
}
