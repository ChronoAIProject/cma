use super::*;
use std::pin::Pin;
use std::task::{Context, Poll};

#[tokio::test]
async fn unrelated_initialize_errors_never_rejoin_even_with_a_held_conversation() {
    for error in [
        serde_json::json!({"code": -32603, "message": "Internal error", "data": {"details": "Invalid clientInfo.name"}}),
        serde_json::json!({"code": 1001, "message": "Codex process has exited with code 1"}),
        serde_json::json!({"code": -32603, "message": "Internal error"}),
        serde_json::json!({"code": -32603, "message": "Internal error", "data": {"details": "Already initialized elsewhere"}}),
        serde_json::json!({"code": -32000, "message": "already initialized"}),
        serde_json::json!({"code": -32600, "message": "Already initialized"}),
    ] {
        let (client, mut server) = socket_pair().await;
        send_ready(&mut server, "agent-1").await;
        let expected = format!("initialize: {error}");
        let serving = tokio::spawn(async move {
            let initialize = receive_json(&mut server).await;
            send_json(
                &mut server,
                serde_json::json!({
                    "jsonrpc": "2.0", "id": initialize["id"], "error": error,
                }),
            )
            .await;
            while let Some(Ok(frame)) = server.next().await {
                assert!(
                    !matches!(frame, Message::Binary(_)),
                    "a refused initialize cannot send session or prompt work"
                );
            }
        });
        let (updates, _receiver) = mpsc::channel(8);
        let result = Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            settings(None, false),
            Some("conversation-held".into()),
            updates,
        )
        .await;
        assert!(matches!(result, Err(AdapterError::Rejected(detail)) if detail == expected));
        serving.await.unwrap();
    }
}

#[tokio::test]
async fn provider_must_explicitly_prove_its_existing_initialize_response() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        reject(&mut server, &initialize).await;
        while let Some(Ok(frame)) = server.next().await {
            assert!(!matches!(frame, Message::Binary(_)));
        }
    });
    let (updates, _receiver) = mpsc::channel(8);
    let mut configured = settings(None, false);
    configured.already_initialized_error = None;
    let result = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        configured,
        Some("conversation-held".into()),
        updates,
    )
    .await;
    assert!(matches!(result, Err(AdapterError::Rejected(_))));
    serving.await.unwrap();
}

struct StalledCloseSocket {
    inner: Socket,
    dropped: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for StalledCloseSocket {
    fn drop(&mut self) {
        let _ = self.dropped.take().unwrap().send(());
    }
}

impl futures_util::Stream for StalledCloseSocket {
    type Item = Result<Message, tokio_tungstenite::tungstenite::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl futures_util::Sink<Message> for StalledCloseSocket {
    type Error = tokio_tungstenite::tungstenite::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.inner).start_send(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Pending
    }
}

#[tokio::test]
async fn failed_open_returns_original_error_without_waiting_for_socket_cleanup() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (dropped, released) = tokio::sync::oneshot::channel();
    let client = Box::new(StalledCloseSocket {
        inner: client,
        dropped: Some(dropped),
    });
    let serving = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        reject(&mut server, &initialize).await;
        while let Some(Ok(frame)) = server.next().await {
            assert!(!matches!(frame, Message::Binary(_)));
        }
    });
    let (updates, _receiver) = mpsc::channel(8);
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        Bridge::open(
            client,
            "agent-1",
            "/workspace".into(),
            settings(None, false),
            None,
            updates,
        ),
    )
    .await
    .expect("cleanup cannot hold the original refusal");
    assert!(
        matches!(result, Err(AdapterError::Rejected(detail)) if detail.contains("held conversation identifier"))
    );
    tokio::time::timeout(Duration::from_secs(1), released)
        .await
        .expect("opening tasks release the socket")
        .unwrap();
    serving.await.unwrap();
}
