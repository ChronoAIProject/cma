//! Bounded, signed delivery with DNS validation and address pinning on each attempt.
use super::intents::DeliveryOutcome;
use crate::auth::webhook;
pub use crate::net::exchange::TransportError;
pub use crate::net::policy::public_ip;
use crate::net::{policy, resolve};
use cmaeg_core::channel::Secret;
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Url};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc, time::Duration};

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
            resolve::resolve(&url).await.map_err(|error| match error {
                resolve::ResolveError::Unavailable => TransportError::Unavailable,
            })
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
            let client = resolve::pinned_client(
                url.host_str().ok_or(TransportError::Unavailable)?,
                &addresses,
                Duration::from_secs(3),
                Duration::from_secs(5),
            )
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
                if !resolve::all_public(&addresses) {
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
