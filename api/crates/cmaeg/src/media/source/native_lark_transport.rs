//! The regional resource API is the only recipient of the native tenant token.
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
            if url.scheme() != "https"
                || !matches!(
                    url.host_str(),
                    Some("open.feishu.cn" | "open.larksuite.com")
                )
                || url.port_or_known_default() != Some(443)
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(FetchError::Rejected);
            }
            let addresses = timeout_at(deadline, resolve(url))
                .await
                .map_err(|_| FetchError::TimedOut)?
                .map_err(|_| FetchError::Upstream)?;
            if !all_public(&addresses) {
                return Err(FetchError::Rejected);
            }
            let client = pinned_client(
                url.host_str().ok_or(FetchError::Rejected)?,
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
