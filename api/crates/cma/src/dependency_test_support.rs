//! Loopback-only HTTP peers with explicit request/response barriers.
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Request, State},
    http::{HeaderMap, Method, Response, Uri},
    Router,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{mpsc, oneshot};

pub(crate) fn assert_deadline(deadline: tokio::time::Instant) {
    // Tokio rounds deadlines up to the next millisecond, even with paused time.
    let overrun = tokio::time::Instant::now().saturating_duration_since(deadline);
    assert!(
        overrun <= std::time::Duration::from_millis(1),
        "deadline overrun: {overrun:?}"
    );
}

pub(crate) async fn finish<T>(mut task: tokio::task::JoinHandle<T>) -> T {
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), &mut task).await;
    if result.is_err() {
        task.abort();
    }
    result
        .expect("dependency returns within the fixture watchdog")
        .unwrap()
}

pub(crate) struct Exchange {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: Bytes,
    reply: oneshot::Sender<Response<Body>>,
}

impl Exchange {
    pub fn respond(self, status: u16, body: impl Into<Body>) {
        self.reply
            .send(
                Response::builder()
                    .status(status)
                    .body(body.into())
                    .unwrap(),
            )
            .expect("fixture response receiver remains alive");
    }

    pub fn json(self, value: serde_json::Value) {
        self.respond(200, value.to_string());
    }

    pub fn stall_body(self) {
        self.respond(
            200,
            Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::io::Error>>()),
        );
    }
}

pub(crate) struct Peer {
    pub origin: String,
    inbox: mpsc::Receiver<Exchange>,
    seen: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Peer {
    pub async fn start() -> Self {
        let (sender, inbox) = mpsc::channel(64);
        let seen = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .fallback(receive)
            .with_state((sender, seen.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            origin,
            inbox,
            seen,
            task,
        }
    }

    pub async fn next(&mut self) -> Exchange {
        tokio::time::timeout(std::time::Duration::from_secs(2), self.inbox.recv())
            .await
            .expect("local request arrives within fixture watchdog")
            .expect("local peer remains open")
    }

    pub fn count(&self) -> usize {
        self.seen.load(Ordering::SeqCst)
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type PeerState = (mpsc::Sender<Exchange>, Arc<AtomicUsize>);

async fn receive(State((sender, seen)): State<PeerState>, request: Request) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 1024 * 1024).await.unwrap();
    let (reply, response) = oneshot::channel();
    seen.fetch_add(1, Ordering::SeqCst);
    if sender
        .send(Exchange {
            method: parts.method,
            uri: parts.uri,
            headers: parts.headers,
            body,
            reply,
        })
        .await
        .is_err()
    {
        return Response::new(Body::empty());
    }
    response
        .await
        .unwrap_or_else(|_| Response::new(Body::empty()))
}
