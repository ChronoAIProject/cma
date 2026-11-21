use std::time::Duration;

use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

pub(crate) const DISPLAY_URL: &str = "ws://127.0.0.1:6080/websockify";

pub(crate) type Display =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub(crate) async fn connect(token: &str, display: &str) -> Result<Display, ()> {
    let config = WebSocketConfig {
        max_message_size: Some(8 * 1024 * 1024),
        max_frame_size: Some(8 * 1024 * 1024),
        ..WebSocketConfig::default()
    };
    let mut request = display.into_client_request().map_err(|_| ())?;
    let mut credential: axum::http::HeaderValue = token.parse().map_err(|_| ())?;
    credential.set_sensitive(true);
    request.headers_mut().insert("host", credential);
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async_with_config(request, Some(config), true),
    )
    .await
    .map_err(|_| ())?
    .map(|(upstream, _)| upstream)
    .map_err(|_| ())
}

#[cfg(any(target_os = "linux", test))]
pub(super) async fn authenticate(token: &str, display: &str) -> Result<(), ()> {
    use futures_util::StreamExt as _;
    use tokio_tungstenite::tungstenite::Message;

    let mut stream = connect(token, display).await?;
    // The VNC version greeting proves the authenticated upgrade reached the
    // display behind websockify. An unrelated TCP listener cannot supply it.
    let mut banner = [0; 12];
    let mut received = 0;
    while received < banner.len() {
        match stream.next().await {
            Some(Ok(Message::Binary(bytes))) => {
                let count = bytes.len().min(banner.len() - received);
                banner[received..received + count].copy_from_slice(&bytes[..count]);
                received += count;
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            _ => return Err(()),
        }
    }
    if &banner[..4] == b"RFB "
        && banner[7] == b'.'
        && banner[11] == b'\n'
        && banner[4..7]
            .iter()
            .chain(&banner[8..11])
            .all(u8::is_ascii_digit)
    {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::SinkExt as _;
    use tokio_tungstenite::tungstenite::handshake::server::{
        Callback, ErrorResponse, Request, Response,
    };
    use tokio_tungstenite::tungstenite::Message;

    struct CurrentToken;

    impl Callback for CurrentToken {
        fn on_request(
            self,
            request: &Request,
            response: Response,
        ) -> Result<Response, ErrorResponse> {
            if request
                .headers()
                .get("host")
                .is_some_and(|host| host == "current")
            {
                Ok(response)
            } else {
                Err(axum::http::Response::builder()
                    .status(403)
                    .body(None)
                    .unwrap())
            }
        }
    }

    #[tokio::test]
    async fn authenticating_a_generation_requires_its_token_and_a_display_greeting() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = format!("ws://{}/display", listener.local_addr().unwrap());
        let listener = tokio::spawn(async move {
            for greeting in [None, Some("wrong-format"), Some("RFB 003.008\n")] {
                let (socket, _) = listener.accept().await.unwrap();
                let upgraded = tokio_tungstenite::accept_hdr_async(socket, CurrentToken).await;
                if let Some(greeting) = greeting {
                    let mut stream = upgraded.unwrap();
                    for chunk in greeting.as_bytes().chunks(4) {
                        stream.send(Message::Binary(chunk.to_vec())).await.unwrap();
                    }
                } else {
                    assert!(upgraded.is_err());
                }
            }
        });
        assert!(authenticate("previous", &target).await.is_err());
        assert!(authenticate("current", &target).await.is_err());
        assert!(authenticate("current", &target).await.is_ok());
        listener.await.unwrap();
    }
}
