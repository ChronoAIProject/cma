use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use cmaeg::delivery::webhook_client::{TransportError, WebhookTransport};
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::task::JoinHandle;
pub struct Received {
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}
#[derive(Clone)]
struct Data {
    received: Arc<Mutex<Vec<Received>>>,
    failures: Arc<AtomicUsize>,
}
pub struct Receiver {
    pub received: Arc<Mutex<Vec<Received>>>,
    pub transport: Arc<LocalTransport>,
    task: JoinHandle<()>,
}
pub struct LocalTransport {
    base: String,
    pub resolved: Mutex<Vec<SocketAddr>>,
    pub lookups: AtomicUsize,
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Receiver {
    pub async fn new(failures: usize) -> Self {
        let data = Data {
            received: Arc::new(Mutex::new(vec![])),
            failures: Arc::new(AtomicUsize::new(failures)),
        };
        let received = data.received.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/events", post(record))
            .with_state(data);
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            received,
            transport: Arc::new(LocalTransport {
                base,
                resolved: Mutex::new(vec!["8.8.8.8:443".parse().unwrap()]),
                lookups: 0.into(),
            }),
            task,
        }
    }
    pub fn events(&self) -> Vec<Value> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .map(|request| serde_json::from_slice(&request.body).unwrap())
            .collect()
    }
}
async fn record(
    State(data): State<Data>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> StatusCode {
    data.received.lock().unwrap().push(Received {
        headers,
        body: body.to_vec(),
    });
    if data
        .failures
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
            count.checked_sub(1)
        })
        .is_ok()
    {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::OK
    }
}
impl WebhookTransport for LocalTransport {
    fn resolve(&self, url: reqwest::Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        Box::pin(async move {
            assert_eq!(url.host_str(), Some("receiver.example"));
            self.lookups.fetch_add(1, Ordering::SeqCst);
            Ok(self.resolved.lock().unwrap().clone())
        })
    }
    fn post(
        &self,
        url: reqwest::Url,
        addresses: Vec<SocketAddr>,
        headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        Box::pin(async move {
            assert_eq!(url.as_str(), "https://receiver.example/events");
            assert!(addresses
                .iter()
                .all(|address| cmaeg::delivery::webhook_client::public_ip(address.ip())));
            reqwest::Client::new()
                .post(format!("{}/events", self.base))
                .headers(headers)
                .body(body.as_ref().clone())
                .send()
                .await
                .map(|response| response.status().as_u16())
                .map_err(|_| TransportError::Unavailable)
        })
    }
}
