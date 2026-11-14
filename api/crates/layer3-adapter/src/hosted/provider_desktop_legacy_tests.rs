use super::super::tests::{json_response, listener, read_request, request_text};
use super::*;
use crate::hosted::HostedControlTransport;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::Message;

struct LegacyHandshake(String);

impl Callback for LegacyHandshake {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        assert_eq!(request.uri().path(), "/retained/proxy/runtime/desktop");
        assert_eq!(request.headers()["open-sandbox-api-key"], self.0);
        assert!(!request.headers().contains_key(DESKTOP_OWNER_HEADER));
        assert!(!request.headers().contains_key("x-cma-runtime-token"));
        Ok(response)
    }
}

async fn discover(listener: &tokio::net::TcpListener, endpoint: &str, credential: &str) {
    let (mut stream, _) = listener.accept().await.unwrap();
    let request = request_text(read_request(&mut stream).await);
    assert!(request
        .starts_with("GET /v1/sandboxes/retained-runtime/endpoints/9090?use_server_proxy=true "));
    assert!(request.contains(&format!("open-sandbox-api-key: {credential}\r\n")));
    stream
        .write_all(&json_response(&serde_json::json!({"endpoint": endpoint})))
        .await
        .unwrap();
}

#[tokio::test]
async fn retained_provider_legacy_desktop_authenticates_and_exchanges_frames() {
    let listener = listener().await;
    let address = listener.local_addr().unwrap();
    let credential = uuid::Uuid::new_v4().to_string();
    let transport = ProviderHostedControlTransport::for_retained_opensandbox(SandboxClient::new(
        format!("http://{address}"),
        credential.clone(),
    ))
    .unwrap();
    let server = tokio::spawn(async move {
        discover(
            &listener,
            &format!("http://{address}/retained/proxy"),
            &credential,
        )
        .await;
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_hdr_async(stream, LegacyHandshake(credential))
            .await
            .unwrap();
        for expected in [
            Message::Binary(vec![1, 2, 3]),
            Message::Text("frame".into()),
        ] {
            let frame = socket.next().await.unwrap().unwrap();
            assert_eq!(frame, expected);
            socket.send(frame).await.unwrap();
        }
        assert!(matches!(
            socket.next().await.unwrap().unwrap(),
            Message::Close(_)
        ));
        socket.flush().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let mut connection = transport
        .open_desktop(&RuntimeHandle::new("retained-runtime"))
        .await
        .unwrap_or_else(|error| panic!("legacy provider desktop must open: {error}"));
    assert!(connection.ownership_control().is_none());
    for frame in [
        crate::DesktopFrame::Binary(vec![1, 2, 3]),
        crate::DesktopFrame::Text("frame".into()),
    ] {
        connection.send(frame.clone()).await.unwrap();
        assert_eq!(connection.recv().await.unwrap(), Some(frame));
    }
    connection.send(crate::DesktopFrame::Close).await.unwrap();
    assert_eq!(
        connection.recv().await.unwrap(),
        Some(crate::DesktopFrame::Close)
    );
    server.await.unwrap();
}

#[tokio::test]
async fn retained_provider_legacy_desktop_refuses_without_retry_or_redirect() {
    for status in [
        "401 Unauthorized",
        "409 Conflict",
        "307 Temporary Redirect",
        "lost-ack",
    ] {
        let listener = listener().await;
        let address = listener.local_addr().unwrap();
        let destination = super::super::tests::listener().await;
        let destination_address = destination.local_addr().unwrap();
        let credential = uuid::Uuid::new_v4().to_string();
        let transport = ProviderHostedControlTransport::for_retained_opensandbox(
            SandboxClient::new(format!("http://{address}"), credential.clone()),
        )
        .unwrap();
        let server = tokio::spawn(async move {
            discover(
                &listener,
                &format!("http://{address}/retained/proxy"),
                &credential,
            )
            .await;
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_text(read_request(&mut stream).await);
            assert!(request.starts_with("GET /retained/proxy/runtime/desktop "));
            assert!(request.contains(&format!("open-sandbox-api-key: {credential}\r\n")));
            assert!(!request.contains(DESKTOP_OWNER_HEADER));
            if status != "lost-ack" {
                stream.write_all(format!(
                    "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: 0\r\nLocation: http://{destination_address}/replacement\r\n\r\n"
                ).as_bytes()).await.unwrap();
            }
            drop(stream);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {},
                _ = listener.accept() => panic!("refused desktop must not retry or rediscover"),
                _ = destination.accept() => panic!("desktop must not follow a redirect"),
            }
        });
        assert!(matches!(
            transport
                .open_desktop(&RuntimeHandle::new("retained-runtime"))
                .await,
            Err(AdapterError::Unavailable(_))
        ));
        server.await.unwrap();
    }
}
