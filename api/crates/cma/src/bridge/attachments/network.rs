//! Resolve once, connect only to vetted addresses, retain the TLS origin name.
use super::*;
use reqwest::{header, redirect::Policy, Client, Url};

#[async_trait::async_trait]
pub(super) trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, Unavailable>;
}

pub(super) struct SystemResolver;

#[async_trait::async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, Unavailable> {
        let addresses = tokio::net::lookup_host((host, 443))
            .await
            .map_err(|_| Unavailable::Transport)?;
        // One excess item is retained so the bounded constructor rejects it.
        Ok(addresses.take(17).map(|a| a.ip()).collect())
    }
}

pub(super) struct PinnedTarget {
    pub(super) url: Url,
    pub(super) host: String,
    pub(super) addresses: Vec<SocketAddr>,
    pub(super) deadline: Instant,
}

impl PinnedTarget {
    pub(super) fn new(
        grant: &FetchGrant,
        addresses: Vec<IpAddr>,
        deadline: Instant,
    ) -> Result<Self, Unavailable> {
        if addresses.is_empty()
            || addresses.len() > 16
            || addresses.iter().any(|a| !public_address(*a))
        {
            return Err(Unavailable::Address);
        }
        Ok(Self {
            url: grant.url.clone(),
            host: grant.host.clone(),
            addresses: addresses
                .into_iter()
                .map(|a| SocketAddr::new(a, 443))
                .collect(),
            deadline: deadline.min(grant.deadline),
        })
    }
}

#[async_trait::async_trait]
pub(super) trait DownloadBody: Send {
    /// Reject an oversized transport chunk before copying its bytes.
    async fn next(&mut self, remaining: usize) -> Result<Option<Vec<u8>>, Unavailable>;
}

pub(super) struct DownloadResponse {
    pub status: u16,
    pub peer: SocketAddr,
    pub mime: Option<String>,
    pub encoding: Option<String>,
    pub length: Option<u64>,
    pub body: Box<dyn DownloadBody>,
}

#[async_trait::async_trait]
pub(super) trait Transport: Send + Sync {
    async fn get(&self, target: &PinnedTarget) -> Result<DownloadResponse, Unavailable>;
}

pub(super) struct HttpsTransport;

impl HttpsTransport {
    fn client(target: &PinnedTarget) -> Result<Client, Unavailable> {
        // A fresh pool cannot reuse a connection resolved under an earlier
        // grant. resolve_to_addrs overrides DNS while preserving SNI/cert checks.
        Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(Policy::none())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .resolve_to_addrs(&target.host, &target.addresses)
            .timeout(target.deadline.saturating_duration_since(Instant::now()))
            .build()
            .map_err(|_| Unavailable::Transport)
    }
}

#[async_trait::async_trait]
impl Transport for HttpsTransport {
    async fn get(&self, target: &PinnedTarget) -> Result<DownloadResponse, Unavailable> {
        current(target.deadline)?;
        let response = Self::client(target)?
            .get(target.url.clone())
            .header(header::ACCEPT_ENCODING, "identity")
            // No callback headers, cookies, selected NyxID key or provider
            // credentials are accepted by this transport's request interface.
            .send()
            .await
            .map_err(|_| Unavailable::Transport)?;
        let peer = response.remote_addr().ok_or(Unavailable::Address)?;
        let mime = single_header(&response, header::CONTENT_TYPE)?;
        let encoding = single_header(&response, header::CONTENT_ENCODING)?;
        let length = single_header(&response, header::CONTENT_LENGTH)?
            .map(|value| value.parse::<u64>().map_err(|_| Unavailable::Response))
            .transpose()?;
        Ok(DownloadResponse {
            status: response.status().as_u16(),
            peer,
            mime,
            encoding,
            length,
            body: Box::new(Body(response)),
        })
    }
}

fn single_header(
    response: &reqwest::Response,
    name: header::HeaderName,
) -> Result<Option<String>, Unavailable> {
    let mut values = response.headers().get_all(name).iter();
    let value = values
        .next()
        .map(|v| v.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| Unavailable::Response)?;
    if values.next().is_some() {
        return Err(Unavailable::Response);
    }
    Ok(value)
}

struct Body(reqwest::Response);

#[async_trait::async_trait]
impl DownloadBody for Body {
    async fn next(&mut self, remaining: usize) -> Result<Option<Vec<u8>>, Unavailable> {
        let chunk = self.0.chunk().await.map_err(|_| Unavailable::Transport)?;
        match chunk {
            Some(bytes) if bytes.len() > remaining => Err(Unavailable::Bounds),
            Some(bytes) => Ok(Some(bytes.to_vec())),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reqwest_body_enforces_remaining_budget_before_returning_a_copy() {
        for remaining in [0, 2, 3] {
            let response = axum::http::Response::new(vec![1_u8, 2, 3]);
            let mut body = Body(reqwest::Response::from(response));
            if remaining < 3 {
                assert_eq!(body.next(remaining).await, Err(Unavailable::Bounds));
            } else {
                assert_eq!(body.next(remaining).await, Ok(Some(vec![1, 2, 3])));
                assert_eq!(body.next(0).await, Ok(None));
            }
        }
    }
}
