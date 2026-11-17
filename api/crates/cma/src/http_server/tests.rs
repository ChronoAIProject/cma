use super::*;
use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, State},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::{mpsc, oneshot};

struct Fixture {
    address: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<io::Result<()>>,
}
impl Fixture {
    async fn new(app: Router, age: Option<Duration>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, wait) = oneshot::channel();
        let task = tokio::spawn(serve(
            listener,
            app,
            async {
                let _ = wait.await;
            },
            age,
        ));
        Self {
            address,
            stop: Some(stop),
            task,
        }
    }
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
    async fn shutdown(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), &mut self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn age() -> Option<Duration> {
    Some(Duration::from_millis(250))
}
fn client(h2: bool) -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(4));
    if h2 {
        builder.http2_prior_knowledge()
    } else {
        builder.http1_only()
    }
    .build()
    .unwrap()
}
async fn peer(ConnectInfo(peer): ConnectInfo<SocketAddr>) -> String {
    peer.to_string()
}
async fn text(client: &reqwest::Client, fixture: &Fixture) -> String {
    client
        .get(fixture.url("/peer"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap()
}
async fn retire() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}

#[test]
fn configuration_is_bounded_and_opt_in() {
    assert_eq!(parse_max_age("0").unwrap(), None);
    assert_eq!(parse_max_age("60").unwrap(), Some(Duration::from_secs(60)));
    for invalid in ["", "-1", "1", "29", "3601", "garbage", "1.5"] {
        assert!(parse_max_age(invalid).is_err(), "{invalid}");
    }
}

async fn rotates_without_replaying_mutations(h2: bool) {
    let mutations = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/peer", get(peer))
        .route(
            "/mutation",
            post(|State(count): State<Arc<AtomicUsize>>| async move {
                count.fetch_add(1, Ordering::SeqCst);
                "ok"
            }),
        )
        .with_state(mutations.clone());
    let fixture = Fixture::new(app, age()).await;
    let client = client(h2);
    let before = text(&client, &fixture).await;
    assert_eq!(
        text(&client, &fixture).await,
        before,
        "control must actually reuse TCP"
    );
    retire().await;
    if !h2 {
        let last = client.get(fixture.url("/peer")).send().await.unwrap();
        assert_eq!(last.headers()["connection"], "close");
        assert_eq!(last.text().await.unwrap(), before);
    }
    let after = text(&client, &fixture).await;
    assert_ne!(before, after, "new request must use a fresh TCP connection");
    client
        .post(fixture.url("/mutation"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(mutations.load(Ordering::SeqCst), 1);
    fixture.shutdown().await;
}
#[tokio::test]
async fn http1_rotates_and_preserves_peer_and_single_write() {
    rotates_without_replaying_mutations(false).await;
}
#[tokio::test]
async fn http2_rotates_and_preserves_peer_and_single_write() {
    rotates_without_replaying_mutations(true).await;
}

async fn holds_body(hold: bool) -> (Response, Option<mpsc::Sender<Result<String, Infallible>>>) {
    let (send, receive) = mpsc::channel(2);
    send.send(Ok("data: first\n\n".to_owned())).await.unwrap();
    let stream = futures_util::stream::unfold(receive, |mut receive| async move {
        receive.recv().await.map(|value| (value, receive))
    });
    let response = Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap();
    (response, hold.then_some(send))
}
async fn stream_survives_age(h2: bool) {
    let (response, sender) = holds_body(true).await;
    let response = Arc::new(tokio::sync::Mutex::new(Some(response)));
    let app = Router::new().route("/peer", get(peer)).route(
        "/stream",
        get(move || {
            let response = response.clone();
            async move { response.lock().await.take().unwrap() }
        }),
    );
    let fixture = Fixture::new(app, age()).await;
    let client = client(h2);
    let mut stream = client.get(fixture.url("/stream")).send().await.unwrap();
    assert_eq!(stream.chunk().await.unwrap().unwrap(), "data: first\n\n");
    retire().await;
    // New requests remain available while the old body is still open.
    assert!(!text(&client, &fixture).await.is_empty());
    let sender = sender.unwrap();
    sender.send(Ok("data: last\n\n".into())).await.unwrap();
    drop(sender);
    assert_eq!(stream.chunk().await.unwrap().unwrap(), "data: last\n\n");
    assert!(stream.bytes().await.unwrap().is_empty());
    fixture.shutdown().await;
}
#[tokio::test]
async fn http1_active_response_survives_age() {
    stream_survives_age(false).await;
}
#[tokio::test]
async fn http2_active_stream_survives_goaway() {
    stream_survives_age(true).await;
}

#[tokio::test]
async fn disabled_keeps_the_existing_persistent_connection() {
    let fixture = Fixture::new(Router::new().route("/peer", get(peer)), None).await;
    let client = client(true);
    let before = text(&client, &fixture).await;
    retire().await;
    assert_eq!(text(&client, &fixture).await, before);
    fixture.shutdown().await;
}

#[tokio::test]
async fn upgraded_websocket_survives_age() {
    let app = Router::new().route(
        "/ws",
        get(|ws: WebSocketUpgrade| async {
            ws.on_upgrade(|mut socket| async move {
                while let Some(Ok(message)) = socket.recv().await {
                    if socket.send(message).await.is_err() {
                        break;
                    }
                }
            })
        }),
    );
    let fixture = Fixture::new(app, age()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/ws", fixture.address))
        .await
        .unwrap();
    retire().await;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        "after-age".into(),
    ))
    .await
    .unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap().into_text().unwrap(),
        "after-age"
    );
    ws.close(None).await.unwrap();
    fixture.shutdown().await;
}

#[tokio::test]
async fn process_shutdown_waits_for_active_body() {
    let (response, sender) = holds_body(true).await;
    let response = Arc::new(tokio::sync::Mutex::new(Some(response)));
    let app = Router::new().route(
        "/stream",
        get(move || {
            let response = response.clone();
            async move { response.lock().await.take().unwrap().into_response() }
        }),
    );
    let mut fixture = Fixture::new(app, Some(Duration::from_secs(60))).await;
    let mut stream = client(true)
        .get(fixture.url("/stream"))
        .send()
        .await
        .unwrap();
    stream.chunk().await.unwrap().unwrap();
    fixture.stop.take().unwrap().send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut fixture.task)
            .await
            .is_err()
    );
    drop(sender);
    assert!(stream.bytes().await.unwrap().is_empty());
    tokio::time::timeout(Duration::from_secs(2), &mut fixture.task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn busy_http2_connection_ages_even_when_never_idle() {
    let fixture = Fixture::new(Router::new().route("/peer", get(peer)), age()).await;
    let client = client(true);
    let mut peers = std::collections::HashSet::new();
    // Every gap is much shorter than the age; an idle timeout cannot pass this.
    for _ in 0..12 {
        peers.insert(text(&client, &fixture).await);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(
        peers.len() >= 2,
        "continuous traffic must not extend admission age"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn new_connection_is_selected_again_by_a_tcp_load_balancer() {
    let a = Fixture::new(Router::new().route("/peer", get(|| async { "a" })), age()).await;
    let b = Fixture::new(Router::new().route("/peer", get(|| async { "b" })), age()).await;
    let addresses = [a.address, b.address];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let front = listener.local_addr().unwrap();
    let proxy = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        let mut index = 0;
        loop {
            tokio::select! {
                socket = listener.accept() => {
                    let (mut downstream, _) = socket.unwrap();
                    let address = addresses[index % 2];
                    index += 1;
                    connections.spawn(async move {
                        let mut upstream = tokio::net::TcpStream::connect(address).await.unwrap();
                        let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
                    });
                }
                Some(_) = connections.join_next() => {}
            }
        }
    });
    // Dropping this guard also cleans up the proxy on an assertion failure.
    let _guard = AbortTask(proxy.abort_handle());
    let client = client(true);
    for expected in ["a", "b"] {
        for _ in 0..5 {
            let actual = client
                .get(format!("http://{front}/peer"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            assert_eq!(actual, expected);
        }
        retire().await;
    }
    proxy.abort();
    let _ = proxy.await;
    a.shutdown().await;
    b.shutdown().await;
}
struct AbortTask(tokio::task::AbortHandle);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn writes_across_retirements(h2: bool) {
    let mutations = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/mutation",
            post(|State(count): State<Arc<AtomicUsize>>| async move {
                count.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                "ok"
            }),
        )
        .with_state(mutations.clone());
    let fixture = Fixture::new(app, Some(Duration::from_millis(25))).await;
    // Transport retries explicitly disabled: every success must be the single
    // original attempt, including requests straddling connection retirement.
    let client = client(h2);
    for _ in 0..50 {
        let response = client.post(fixture.url("/mutation")).send().await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "ok");
    }
    assert_eq!(mutations.load(Ordering::SeqCst), 50);
    fixture.shutdown().await;
}
#[tokio::test]
async fn http1_writes_cross_retirement_without_transport_retry() {
    writes_across_retirements(false).await;
}
#[tokio::test]
async fn http2_writes_cross_retirement_without_transport_retry() {
    writes_across_retirements(true).await;
}

#[tokio::test]
async fn http1_upgrade_after_age_preserves_switching_protocols() {
    let app = Router::new().route(
        "/ws",
        get(|ws: WebSocketUpgrade| async {
            ws.on_upgrade(|mut socket| async move {
                while let Some(Ok(message)) = socket.recv().await {
                    if socket.send(message).await.is_err() {
                        break;
                    }
                }
            })
        }),
    );
    let fixture = Fixture::new(app, age()).await;
    let socket = tokio::net::TcpStream::connect(fixture.address)
        .await
        .unwrap();
    retire().await;
    let (mut ws, _) =
        tokio_tungstenite::client_async(format!("ws://{}/ws", fixture.address), socket)
            .await
            .unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        "late-upgrade".into(),
    ))
    .await
    .unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap().into_text().unwrap(),
        "late-upgrade"
    );
    ws.close(None).await.unwrap();
    fixture.shutdown().await;
}
