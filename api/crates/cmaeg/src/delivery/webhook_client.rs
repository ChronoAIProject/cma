//! Bounded, signed delivery with DNS validation and address pinning on each attempt.
use super::{
    gate::{expired, SendGate, Ungated},
    intents::DeliveryOutcome,
};
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
        self.send_gated(destination, secret, event, &Ungated).await
    }
    /// `send`, with each attempt admitted by `gate` before its DNS lookup and
    /// POST, and the delivery committed before the first one.
    pub async fn send_gated(
        &self,
        destination: &str,
        secret: &Secret,
        event: &Value,
        gate: &dyn SendGate,
    ) -> DeliveryOutcome {
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
            let permit = match gate.attempt().await {
                Ok(permit) => permit,
                Err(refusal) => return refusal.outcome(),
            };
            // Commit point (Amendment R3 §R3.1): the first attempt's lookup
            // and POST are this delivery's first side effect.
            if attempt == 0 && !gate.commit() {
                return expired();
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
                if !gate.authorize_webhook(destination, secret).await {
                    return Err("webhook_authority_changed");
                }
                self.transport
                    .post(url.clone(), addresses, headers, bytes.clone())
                    .await
                    .map_err(|_| "webhook_unavailable")
            })
            .await
            .unwrap_or(Err("webhook_unavailable"));
            // Backoff never holds an attempt permit.
            drop(permit);
            gate.progress();
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
                Err("webhook_authority_changed") => return rejected("webhook_authority_changed"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery::gate::recording::RecordingGate;
    use std::{collections::VecDeque, sync::Mutex};

    /// Answers every lookup with a public address and each POST with the next
    /// status, counting POSTs.
    struct Statuses {
        statuses: Mutex<VecDeque<u16>>,
        posts: Mutex<usize>,
    }
    impl WebhookTransport for Statuses {
        fn resolve(&self, _url: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
            Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
        }
        fn post(
            &self,
            _url: Url,
            _addresses: Vec<SocketAddr>,
            _headers: HeaderMap,
            _body: Arc<Vec<u8>>,
        ) -> BoxFuture<'_, Result<u16, TransportError>> {
            *self.posts.lock().unwrap() += 1;
            let status = self.statuses.lock().unwrap().pop_front().unwrap_or(200);
            Box::pin(async move { Ok(status) })
        }
    }
    fn client(statuses: impl IntoIterator<Item = u16>) -> (WebhookClient, Arc<Statuses>) {
        let transport = Arc::new(Statuses {
            statuses: Mutex::new(statuses.into_iter().collect()),
            posts: Mutex::default(),
        });
        (WebhookClient::with_transport(transport.clone()), transport)
    }

    fn event() -> Value {
        serde_json::json!({"id":"event-1","type":"cmaeg.response.completed"})
    }

    #[tokio::test(start_paused = true)]
    async fn each_attempt_is_admitted_and_only_the_first_commits() {
        let (client, transport) = client([500, 500, 500, 200]);
        let gate = RecordingGate::new(true);
        let outcome = client
            .send_gated(
                "https://receiver.example/events",
                &Secret::new("result-key"),
                &event(),
                &gate,
            )
            .await;
        assert_eq!(
            outcome,
            DeliveryOutcome::Confirmed {
                platform_message_id: None
            }
        );
        assert_eq!(*transport.posts.lock().unwrap(), 4);
        assert_eq!(
            gate.timed_calls(),
            [
                ("attempt", 0),
                ("commit", 0),
                ("progress", 0),
                ("attempt", 1),
                ("progress", 1),
                ("attempt", 5),
                ("progress", 5),
                ("attempt", 21),
                ("progress", 21),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_commit_sends_nothing_and_send_stays_ungated() {
        let (client, transport) = client([]);
        let gate = RecordingGate::new(false);
        let outcome = client
            .send_gated(
                "https://receiver.example/events",
                &Secret::new("result-key"),
                &event(),
                &gate,
            )
            .await;
        assert_eq!(outcome, expired());
        assert_eq!(*transport.posts.lock().unwrap(), 0);
        assert_eq!(gate.timed_calls(), [("attempt", 0), ("commit", 0)]);
        let outcome = client
            .send(
                "https://receiver.example/events",
                &Secret::new("result-key"),
                &event(),
            )
            .await;
        assert_eq!(
            outcome,
            DeliveryOutcome::Confirmed {
                platform_message_id: None
            }
        );
        assert_eq!(*transport.posts.lock().unwrap(), 1);
    }
}
