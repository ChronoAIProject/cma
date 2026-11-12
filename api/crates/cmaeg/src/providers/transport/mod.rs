//! Pooled Responses transport with attribution at the actual socket write.
mod body;
mod connect;
mod io;
mod observation;
#[cfg(test)]
mod tests;
use crate::{providers::ProviderError, telemetry::timing::IngressTiming};
use axum::http::{header, Request};
use bytes::Bytes;
use cmaeg_core::seal::TurnTraceContext;
use connect::Connection;
use http_body_util::Full;
use observation::{Observation, Pending};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
pub(super) struct Completion(Arc<AtomicBool>);
impl Completion {
    pub fn complete(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    fn is_complete(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
struct Driver(tokio::task::JoinHandle<()>);
impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct H1 {
    send: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    driver: Arc<Driver>,
    pending: Pending,
}
#[derive(Clone)]
struct H2 {
    send: h2::client::SendRequest<Bytes>,
    driver: Arc<Driver>,
    pending: Pending,
    used: Instant,
}
struct Idle {
    connection: H1,
    at: Instant,
}
pub(crate) struct ResponseTransport {
    origin: url::Url,
    connect_timeout: Duration,
    tls: Arc<rustls::ClientConfig>,
    idle: Arc<Mutex<Vec<Idle>>>,
    multiplexed: tokio::sync::Mutex<Option<H2>>,
    protocol: AtomicU8,
    #[cfg(test)]
    address: Option<std::net::SocketAddr>,
}
impl std::fmt::Debug for ResponseTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResponseTransport")
    }
}
pub(super) struct Response {
    pub response: reqwest::Response,
    pub written_at: Instant,
    pub complete: Completion,
}
impl ResponseTransport {
    pub fn new(origin: url::Url, connect_timeout: Duration) -> Self {
        Self {
            origin,
            connect_timeout,
            tls: connect::tls_config(),
            idle: Default::default(),
            multiplexed: Default::default(),
            protocol: AtomicU8::new(0),
            #[cfg(test)]
            address: None,
        }
    }
    async fn acquire(&self) -> Result<Connection, ProviderError> {
        if self.protocol.load(Ordering::Acquire) == 1 {
            return self.acquire_h1().await;
        }
        // Only initial negotiation / HTTP/2 reconnect is single-flight. Established
        // HTTP/1 pools create concurrent connections; HTTP/2 streams share one socket.
        let mut shared = self.multiplexed.lock().await;
        if self.protocol.load(Ordering::Acquire) == 1 {
            drop(shared);
            return self.acquire_h1().await;
        }
        if let Some(connection) = shared.as_mut() {
            let idle_expired = connection.used.elapsed() > Duration::from_secs(90)
                && Arc::strong_count(&connection.driver) == 1;
            if !idle_expired && connection.send.clone().ready().await.is_ok() {
                connection.used = Instant::now();
                return Ok(Connection::H2(connection.clone()));
            }
        }
        *shared = None;
        match self.connect().await? {
            Connection::H2(connection) => {
                self.protocol.store(2, Ordering::Release);
                *shared = Some(connection.clone());
                Ok(Connection::H2(connection))
            }
            Connection::H1(connection) => {
                self.protocol.store(1, Ordering::Release);
                Ok(Connection::H1(connection))
            }
        }
    }
    async fn acquire_h1(&self) -> Result<Connection, ProviderError> {
        loop {
            let candidate = self.idle.lock().unwrap().pop();
            let Some(mut idle) = candidate else {
                return self.connect().await;
            };
            if idle.at.elapsed() < Duration::from_secs(90)
                && idle.connection.send.ready().await.is_ok()
            {
                return Ok(Connection::H1(idle.connection));
            }
        }
    }
    pub async fn send(
        &self,
        url: url::Url,
        key: &str,
        idempotency: &str,
        json: Vec<u8>,
        timing: Option<IngressTiming>,
    ) -> Result<Response, ProviderError> {
        self.send_with_trace(url, key, idempotency, json, timing, None)
            .await
    }

    pub async fn send_with_trace(
        &self,
        url: url::Url,
        key: &str,
        idempotency: &str,
        json: Vec<u8>,
        timing: Option<IngressTiming>,
        turn_trace: Option<&TurnTraceContext>,
    ) -> Result<Response, ProviderError> {
        // The registry builds the URL. Keep an additional origin fence at the socket layer.
        if url.origin() != self.origin.origin() {
            return Err(ProviderError::InvalidRequest(
                "provider origin mismatch".into(),
            ));
        }
        let observation = Observation::new(timing);
        let complete = Completion::default();
        let bytes = Bytes::from(json);
        let mut authorization = axum::http::HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| ProviderError::InvalidRequest("invalid provider authorization".into()))?;
        authorization.set_sensitive(true);
        let mut request = Request::builder()
            .method("POST")
            .uri(url.as_str())
            .header(header::AUTHORIZATION, authorization)
            .header(header::ACCEPT, "text/event-stream")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONTENT_LENGTH, bytes.len())
            .header(
                header::USER_AGENT,
                concat!("cmaeg/", env!("CARGO_PKG_VERSION")),
            )
            .header("x-request-id", uuid::Uuid::new_v4().to_string())
            .header("idempotency-key", idempotency);
        if let Some(turn_trace) = turn_trace {
            if let Ok(value) = crate::telemetry::turn_trace::child_traceparent(turn_trace)
                .parse::<axum::http::HeaderValue>()
            {
                request = request.header("traceparent", value);
                request = request.header("x-nyxid-turn-trace-contract", "1");
            }
        }
        let response = match self.acquire().await? {
            Connection::H1(mut connection) => {
                let _keep_alive = &connection.driver;
                let authority = url[url::Position::BeforeHost..url::Position::AfterPort].to_owned();
                let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
                let request = request
                    .uri(path)
                    .header(header::HOST, authority)
                    .body(Full::new(bytes))
                    .map_err(|_| {
                        ProviderError::InvalidRequest("invalid provider headers".into())
                    })?;
                connection
                    .pending
                    .lock()
                    .unwrap()
                    .insert(0, observation.clone());
                let response = connection
                    .send
                    .send_request(request)
                    .await
                    .map_err(|_| unavailable())?;
                let (parts, body) = response.into_parts();
                let stream = body::H1Body {
                    body: Some(body),
                    connection: Some(connection),
                    idle: self.idle.clone(),
                    complete: complete.clone(),
                };
                reqwest::Response::from(axum::http::Response::from_parts(
                    parts,
                    reqwest::Body::wrap_stream(stream),
                ))
            }
            Connection::H2(connection) => {
                let mut sender = connection
                    .send
                    .clone()
                    .ready()
                    .await
                    .map_err(|_| unavailable())?;
                let request = request.body(()).map_err(|_| {
                    ProviderError::InvalidRequest("invalid provider headers".into())
                })?;
                let (response, mut send, stream_id) = {
                    // Register before another thread's IO driver can inspect HEADERS.
                    let mut pending = connection.pending.lock().unwrap();
                    let (response, send) = sender
                        .send_request(request, false)
                        .map_err(|_| unavailable())?;
                    let stream_id = response.stream_id().as_u32();
                    pending.insert(stream_id, observation.clone());
                    (response, send, stream_id)
                };
                let _registration = Registration {
                    pending: connection.pending.clone(),
                    id: stream_id,
                };
                send.send_data(bytes, true).map_err(|_| unavailable())?;
                let response = response.await.map_err(|_| unavailable())?;
                let (parts, body) = response.into_parts();
                let stream = body::H2Body {
                    body,
                    send,
                    driver: connection.driver,
                };
                reqwest::Response::from(axum::http::Response::from_parts(
                    parts,
                    reqwest::Body::wrap_stream(stream),
                ))
            }
        };
        let written_at =
            observation.at.get().copied().ok_or_else(|| {
                ProviderError::Unavailable("provider write was not observed".into())
            })?;
        Ok(Response {
            response,
            written_at,
            complete,
        })
    }
}
struct Registration {
    pending: Pending,
    id: u32,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}
fn unavailable() -> ProviderError {
    ProviderError::Unavailable("provider transport failed".into())
}
