//! Real TCP HTTP/1 and HTTP/2 connections against the bounded-age server.
use super::*;
use axum::body::Bytes;
use axum::{body::Body, routing::get};
use http_body_util::{BodyExt as _, Empty};
use hyper::Request;
use std::{net::SocketAddr, time::Instant};
use tokio::net::TcpStream;

const AGE: Duration = Duration::from_millis(200);

fn app() -> Router {
    Router::new()
        .route(
            "/peer",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.to_string() }),
        )
        .route(
            "/slow",
            get(|| async {
                // A response that is still streaming when the connection ages out.
                let chunks = futures_util::stream::unfold(0_u8, |sent| async move {
                    (sent < 5).then_some(())?;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Some((
                        Ok::<_, std::io::Error>(Bytes::from_static(b"chunk;")),
                        sent + 1,
                    ))
                });
                Body::from_stream(chunks)
            }),
        )
}

struct Server {
    address: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

async fn start(age: Duration) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(serve(listener, app(), age, async {
        let _ = stopped.await;
    }));
    Server {
        address,
        stop: Some(stop),
        task,
    }
}

fn get_request(path: &str) -> Request<Empty<Bytes>> {
    Request::get(path)
        .header("host", "localhost")
        .body(Empty::new())
        .unwrap()
}

async fn http1(
    address: SocketAddr,
) -> (
    hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    SocketAddr,
    tokio::task::JoinHandle<()>,
) {
    let stream = TcpStream::connect(address).await.unwrap();
    let local = stream.local_addr().unwrap();
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    (
        sender,
        local,
        tokio::spawn(async move {
            let _ = connection.await;
        }),
    )
}

async fn http2(
    address: SocketAddr,
) -> (
    hyper::client::conn::http2::SendRequest<Empty<Bytes>>,
    tokio::task::JoinHandle<()>,
) {
    let stream = TcpStream::connect(address).await.unwrap();
    let (sender, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
            .await
            .unwrap();
    (
        sender,
        tokio::spawn(async move {
            let _ = connection.await;
        }),
    )
}

async fn text(response: hyper::Response<hyper::body::Incoming>) -> String {
    assert!(response.status().is_success());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(body.to_vec()).unwrap()
}

#[tokio::test]
async fn http1_keep_alive_connections_close_after_their_age_and_new_ones_are_admitted() {
    let server = start(AGE).await;
    let (mut sender, local, connection) = http1(server.address).await;
    // The peer address reaches handlers exactly as with axum::serve.
    assert_eq!(
        text(sender.send_request(get_request("/peer")).await.unwrap()).await,
        local.to_string()
    );
    tokio::time::timeout(Duration::from_secs(2), connection)
        .await
        .expect("an idle aged connection is closed")
        .unwrap();
    assert!(sender.ready().await.is_err());
    let (mut fresh, fresh_local, _connection) = http1(server.address).await;
    assert_eq!(
        text(fresh.send_request(get_request("/peer")).await.unwrap()).await,
        fresh_local.to_string()
    );
}

#[tokio::test]
async fn http1_response_in_progress_finishes_before_its_aged_connection_closes() {
    let server = start(AGE).await;
    let (mut sender, _, connection) = http1(server.address).await;
    let started = Instant::now();
    let body = text(sender.send_request(get_request("/slow")).await.unwrap()).await;
    assert_eq!(body, "chunk;".repeat(5));
    assert!(
        started.elapsed() > AGE,
        "the response outlived the connection age"
    );
    tokio::time::timeout(Duration::from_secs(2), connection)
        .await
        .expect("the connection closes after its response")
        .unwrap();
}

#[tokio::test]
async fn http2_open_streams_finish_after_goaway_and_new_streams_need_a_new_connection() {
    let server = start(AGE).await;
    let (mut sender, connection) = http2(server.address).await;
    let streaming = sender.send_request(get_request("/slow")).await.unwrap();
    tokio::time::sleep(AGE * 2).await;
    // GOAWAY was sent while the stream was open; the stream still completes.
    assert_eq!(text(streaming).await, "chunk;".repeat(5));
    assert!(sender.send_request(get_request("/peer")).await.is_err());
    tokio::time::timeout(Duration::from_secs(2), connection)
        .await
        .expect("the HTTP/2 connection ends after its last stream")
        .unwrap();
    let (mut fresh, _connection) = http2(server.address).await;
    assert!(
        !text(fresh.send_request(get_request("/peer")).await.unwrap())
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn shutdown_stops_admission_and_waits_for_responses_in_progress() {
    let mut server = start(Duration::from_secs(3600)).await;
    let (mut sender, _, _connection) = http1(server.address).await;
    let response = tokio::spawn(async move {
        text(sender.send_request(get_request("/slow")).await.unwrap()).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    server.stop.take().unwrap().send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !server.task.is_finished(),
        "serve waits for the open response"
    );
    assert!(TcpStream::connect(server.address).await.is_err());
    assert_eq!(response.await.unwrap(), "chunk;".repeat(5));
    tokio::time::timeout(Duration::from_secs(2), &mut server.task)
        .await
        .expect("serve returns once every connection has finished")
        .unwrap()
        .unwrap();
}

#[test]
fn retirement_jitter_stays_within_ten_percent() {
    let age = Duration::from_secs(300);
    let ages: Vec<_> = (0..22).map(|accepted| jittered(age, accepted)).collect();
    assert!(ages
        .iter()
        .all(|value| *value >= age && *value <= age.mul_f64(1.1)));
    assert!(ages.iter().any(|value| *value > age));
}
