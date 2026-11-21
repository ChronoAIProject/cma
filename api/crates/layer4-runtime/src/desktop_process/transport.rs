use std::time::Duration;

#[cfg(target_os = "linux")]
use std::{fs::File, path::Path, sync::Arc};

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use super::DesktopGeneration;

#[cfg(target_os = "linux")]
mod owned_listener;
#[cfg(target_os = "linux")]
pub(super) use owned_listener::OwnedListener;
#[cfg(target_os = "linux")]
pub(super) mod token_map;

#[cfg(target_os = "linux")]
pub(super) struct DisplayBinding {
    proxy: OwnedListener,
    backend: Arc<OwnedListener>,
}

pub(crate) const DISPLAY_URL: &str = "ws://127.0.0.1:6080/websockify";

pub(crate) trait DisplayIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> DisplayIo for T {}

pub(crate) type Display = tokio_tungstenite::WebSocketStream<Box<dyn DisplayIo>>;

pub(crate) async fn connect(generation: &DesktopGeneration, display: &str) -> Result<Display, ()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        #[cfg(target_os = "linux")]
        if let Some(binding) = &generation.listener {
            binding.backend.check_current().map_err(|_| ())?;
            let socket = binding.proxy.connect().await.map_err(|_| ())?;
            let upstream = handshake(socket, generation.token(), DISPLAY_URL).await?;
            binding.backend.check_current().map_err(|_| ())?;
            return Ok(upstream);
        }
        let uri: axum::http::Uri = display.parse().map_err(|_| ())?;
        let authority = uri.authority().ok_or(())?;
        let socket = tokio::net::TcpStream::connect(authority.as_str())
            .await
            .map_err(|_| ())?;
        socket.set_nodelay(true).map_err(|_| ())?;
        handshake(socket, generation.token(), display).await
    })
    .await
    .map_err(|_| ())?
}

async fn handshake(
    socket: impl DisplayIo + 'static,
    token: &str,
    display: &str,
) -> Result<Display, ()> {
    let config = WebSocketConfig {
        max_message_size: Some(8 * 1024 * 1024),
        max_frame_size: Some(8 * 1024 * 1024),
        ..WebSocketConfig::default()
    };
    let mut request = display.into_client_request().map_err(|_| ())?;
    let mut credential: axum::http::HeaderValue = token.parse().map_err(|_| ())?;
    credential.set_sensitive(true);
    request.headers_mut().insert("host", credential);
    tokio_tungstenite::client_async_with_config(
        request,
        Box::new(socket) as Box<dyn DisplayIo>,
        Some(config),
    )
    .await
    .map(|(upstream, _)| upstream)
    .map_err(|_| ())
}

#[cfg(target_os = "linux")]
pub(super) async fn authenticate(
    token: &str,
    directory: &Path,
    leader: i32,
    token_map: &File,
    backend: &mut Option<Arc<OwnedListener>>,
) -> Result<DisplayBinding, ()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        if backend.is_none() {
            let path = directory.join("rfb.sock");
            let (bound, _) = OwnedListener::discover(&path, leader)
                .await
                .map_err(|_| ())?;
            // Each websockify worker opens this descriptor-backed address.
            // Replacing the pathname cannot retarget those connections.
            token_map::seal(token_map, token, &bound.pinned_endpoint()).map_err(|_| ())?;
            *backend = Some(Arc::new(bound));
        }
        let backend = backend.as_ref().expect("pinned backend").clone();
        backend.check_current().map_err(|_| ())?;
        let published = directory.join("token-ready");
        if !published.is_file() {
            std::fs::File::create(published).map_err(|_| ())?;
        }
        let (proxy, socket) = OwnedListener::discover(&directory.join("display.sock"), leader)
            .await
            .map_err(|_| ())?;
        let mut stream = handshake(socket, token, DISPLAY_URL).await?;
        display_greeting(&mut stream).await?;
        backend.check_current().map_err(|_| ())?;
        Ok(DisplayBinding { proxy, backend })
    })
    .await
    .map_err(|_| ())?
}

#[cfg(any(target_os = "linux", test))]
async fn display_greeting(stream: &mut Display) -> Result<(), ()> {
    use futures_util::StreamExt as _;
    use tokio_tungstenite::tungstenite::Message;

    // The peer is already verified. The VNC greeting confirms that its
    // authenticated proxy can also reach the display it serves.
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
        let old = DesktopGeneration::legacy("previous".into());
        let current = DesktopGeneration::legacy("current".into());
        assert!(connect(&old, &target).await.is_err());
        assert!(
            display_greeting(&mut connect(&current, &target).await.unwrap())
                .await
                .is_err()
        );
        assert!(
            display_greeting(&mut connect(&current, &target).await.unwrap())
                .await
                .is_ok()
        );
        listener.await.unwrap();
    }
}
