//! Authenticated requests to exact Meta origins, without proxy or redirects.
use crate::{
    media::{body::transport, FetchError},
    net::resolve::{all_public, pinned_client, resolve},
};
use cmaeg_core::channel::Secret;
use futures_util::future::BoxFuture;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    Url,
};
use std::time::Duration;
use tokio::time::{timeout_at, Instant};

pub(super) trait Transport: Send + Sync {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        token: &'a Secret,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<reqwest::Response, FetchError>>;
}

pub(super) struct Pinned;
impl Transport for Pinned {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        token: &'a Secret,
        deadline: Instant,
    ) -> BoxFuture<'a, Result<reqwest::Response, FetchError>> {
        Box::pin(async move {
            if !official_origin(url) {
                return Err(FetchError::Rejected);
            }
            let host = url.host_str().ok_or(FetchError::Rejected)?;
            let addresses = timeout_at(deadline, resolve(url))
                .await
                .map_err(|_| FetchError::TimedOut)?
                .map_err(|_| FetchError::Upstream)?;
            if !all_public(&addresses) {
                return Err(FetchError::Rejected);
            }
            let client = pinned_client(
                host,
                &addresses,
                Duration::from_secs(3),
                deadline.saturating_duration_since(Instant::now()),
            )
            .map_err(transport)?;
            let mut auth = HeaderValue::from_str(&format!("Bearer {}", token.as_str()))
                .map_err(|_| FetchError::Unauthorized)?;
            auth.set_sensitive(true);
            timeout_at(
                deadline,
                client.get(url.clone()).header(AUTHORIZATION, auth).send(),
            )
            .await
            .map_err(|_| FetchError::TimedOut)?
            .map_err(transport)
        })
    }
}

fn official_origin(url: &Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("graph.facebook.com" | "lookaside.fbsbx.com")
        )
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

pub(super) fn download_url(raw: &str, media_id: &str) -> Result<Url, FetchError> {
    if raw.len() > 2048 || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(FetchError::Rejected);
    }
    let url = Url::parse(raw).map_err(|_| FetchError::Rejected)?;
    let mut ids = url
        .query_pairs()
        .filter(|(key, _)| key == "mid")
        .map(|(_, value)| value);
    if !official_origin(&url)
        || url.host_str() != Some("lookaside.fbsbx.com")
        || url.path() != "/whatsapp_business/attachments/"
        || ids.next().as_deref() != Some(media_id)
        || ids.next().is_some()
    {
        return Err(FetchError::Rejected);
    }
    Ok(url)
}

pub(super) fn status(status: reqwest::StatusCode) -> Result<(), FetchError> {
    match status.as_u16() {
        200 => Ok(()),
        401 | 403 => Err(FetchError::Unauthorized),
        404 => Err(FetchError::NotFound),
        429 => Err(FetchError::RateLimited),
        503 => Err(FetchError::Busy),
        500..=599 => Err(FetchError::Upstream),
        _ => Err(FetchError::Rejected),
    }
}
