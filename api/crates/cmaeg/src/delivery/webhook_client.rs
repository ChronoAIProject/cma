//! Bounded, signed delivery with DNS validation and address pinning on each attempt.
use super::intents::DeliveryOutcome;
use crate::auth::webhook;
use cmaeg_core::channel::Secret;
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Url};
use serde_json::Value;
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
mod policy;
pub use policy::public_ip;

#[derive(Debug)]
pub enum TransportError {
    Unavailable,
}
/// Replaceable IO boundary for local integration fixtures; destination policy remains in the client.
pub trait WebhookTransport: Send + Sync {
    fn resolve(&self, url: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>>;
    fn post(
        &self,
        url: Url,
        addresses: Vec<SocketAddr>,
        headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>>;
}
pub struct HttpTransport;
impl WebhookTransport for HttpTransport {
    fn resolve(&self, url: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        Box::pin(async move {
            let port = url
                .port_or_known_default()
                .ok_or(TransportError::Unavailable)?;
            match url.host().ok_or(TransportError::Unavailable)? {
                url::Host::Ipv4(ip) => Ok(vec![SocketAddr::new(IpAddr::V4(ip), port)]),
                url::Host::Ipv6(ip) => Ok(vec![SocketAddr::new(IpAddr::V6(ip), port)]),
                url::Host::Domain(host) => Ok(tokio::net::lookup_host((host, port))
                    .await
                    .map_err(|_| TransportError::Unavailable)?
                    .take(33)
                    .collect()),
            }
        })
    }
    fn post(
        &self,
        url: Url,
        addresses: Vec<SocketAddr>,
        headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        Box::pin(async move {
            let client = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .resolve_to_addrs(
                    url.host_str().ok_or(TransportError::Unavailable)?,
                    &addresses,
                )
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|_| TransportError::Unavailable)?;
            let response = client
                .post(url)
                .headers(headers)
                .body(body.as_ref().clone())
                .send()
                .await
                .map_err(|_| TransportError::Unavailable)?;
            Ok(response.status().as_u16())
        })
    }
}
#[derive(Clone)]
pub struct WebhookClient {
    transport: Arc<dyn WebhookTransport>,
}
impl Default for WebhookClient {
    fn default() -> Self {
        Self::with_transport(Arc::new(HttpTransport))
    }
}
impl WebhookClient {
    pub fn with_transport(transport: Arc<dyn WebhookTransport>) -> Self {
        Self { transport }
    }
    pub async fn send(&self, destination: &str, secret: &Secret, event: &Value) -> DeliveryOutcome {
        let Some(url) = policy::target(destination) else {
            return rejected("webhook_target_invalid");
        };
        let Some(id) = event
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 512)
        else {
            return rejected("webhook_event_invalid");
        };
        let Ok(bytes) = serde_json::to_vec(event) else {
            return rejected("webhook_event_invalid");
        };
        if bytes.len() > 1048576 || secret.as_str().is_empty() {
            return rejected("webhook_event_invalid");
        }
        let bytes = Arc::new(bytes);
        for (attempt, delay) in [0, 1, 4, 16].into_iter().enumerate() {
            if delay > 0 {
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                let addresses = self
                    .transport
                    .resolve(url.clone())
                    .await
                    .map_err(|_| "webhook_unavailable")?;
                if addresses.is_empty()
                    || addresses.len() > 32
                    || addresses.iter().any(|address| !public_ip(address.ip()))
                {
                    return Err("webhook_target_invalid");
                }
                let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
                let mut headers = HeaderMap::new();
                for (name, value) in [
                    ("content-type", "application/cloudevents+json".into()),
                    ("webhook-id", id.to_owned()),
                    ("webhook-timestamp", timestamp.clone()),
                    (
                        "webhook-signature",
                        webhook::signature(secret, id, &timestamp, &bytes),
                    ),
                ] {
                    headers.insert(name, value.parse().map_err(|_| "webhook_event_invalid")?);
                }
                self.transport
                    .post(url.clone(), addresses, headers, bytes.clone())
                    .await
                    .map_err(|_| "webhook_unavailable")
            })
            .await
            .unwrap_or(Err("webhook_unavailable"));
            tracing::info!(
                attempt,
                confirmed = matches!(result, Ok(200..=299)),
                "result webhook attempt finished"
            );
            match result {
                Ok(200..=299) => {
                    return DeliveryOutcome::Confirmed {
                        platform_message_id: None,
                    }
                }
                Ok(400..=499) if result != Ok(429) => return rejected("webhook_rejected"),
                Ok(300..=399) => return rejected("webhook_redirect_refused"),
                Err("webhook_target_invalid") => return rejected("webhook_target_invalid"),
                Err("webhook_event_invalid") => return rejected("webhook_event_invalid"),
                _ => {}
            }
        }
        DeliveryOutcome::Unknown {
            reason: "webhook_unavailable".into(),
        }
    }
}
fn rejected(reason: &str) -> DeliveryOutcome {
    DeliveryOutcome::Rejected {
        reason: reason.into(),
    }
}
