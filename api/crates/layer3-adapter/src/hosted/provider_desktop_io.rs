use super::*;
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use tokio_tungstenite::tungstenite::Message;

impl Captured {
    pub(super) async fn current(
        &self,
        transport: &ProviderHostedControlTransport,
    ) -> crate::Result<()> {
        let (identity, _) = transport.native_endpoint_state(&self.handle).await;
        if identity != self.identity {
            return Err(refused());
        }
        if let Some(pod) = &self.pod {
            let RuntimeDiscovery::AgentSandbox(client) = &transport.client else {
                return Err(refused());
            };
            if client.pod_binding(&self.handle).await? != *pod {
                return Err(refused());
            }
        }
        if transport.native_endpoint_state(&self.handle).await.0 != self.identity {
            return Err(refused());
        }
        Ok(())
    }

    pub(super) async fn exchange<T: Serialize + Sync, R: DeserializeOwned>(
        &self,
        transport: &ProviderHostedControlTransport,
        action: &str,
        request: &T,
    ) -> crate::Result<R> {
        self.current(transport).await?;
        let url = self.endpoint.desktop_ownership(action)?;
        let bytes = if self.endpoint.native_binding().is_some() {
            let response = transport
                .native_http_for(
                    &self.endpoint,
                    http::Method::POST,
                    url,
                    serde_json::to_vec(request).map_err(|_| refused())?,
                    8192,
                    (
                        native_pool::NativeHttpLane::Control,
                        RuntimeWorkloadClass::Control,
                    ),
                )
                .await?;
            if !response.status.is_success() {
                return Err(refused());
            }
            response.recycle().await?
        } else {
            let (name, credential) = transport.auth_header(&self.endpoint);
            let mut response = transport
                .desktop_http
                .post(url)
                .header(name, credential)
                .json(request)
                .send()
                .await
                .map_err(|_| refused())?;
            if !response.status().is_success() {
                return Err(refused());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| refused())? {
                if chunk.len() > 8192_usize.saturating_sub(bytes.len()) {
                    return Err(refused());
                }
                bytes.extend_from_slice(&chunk);
            }
            bytes
        };
        self.current(transport).await?;
        serde_json::from_slice(&bytes).map_err(|_| refused())
    }

    pub(super) async fn connect(
        &self,
        transport: &ProviderHostedControlTransport,
        owner: &DesktopOwnership,
        control: Arc<dyn DesktopOwnershipControl>,
    ) -> crate::Result<crate::DesktopConnectionHandle> {
        self.current(transport).await?;
        let mut request = self
            .endpoint
            .desktop()?
            .as_str()
            .into_client_request()
            .map_err(|_| refused())?;
        let (name, credential) = transport.auth_header(&self.endpoint);
        let mut credential = http::HeaderValue::from_str(credential).map_err(|_| refused())?;
        credential.set_sensitive(true);
        request.headers_mut().insert(name, credential);
        let mut header =
            http::HeaderValue::from_str(&serde_json::to_string(owner).map_err(|_| refused())?)
                .map_err(|_| refused())?;
        header.set_sensitive(true);
        request.headers_mut().insert(DESKTOP_OWNER_HEADER, header);
        let config = WebSocketConfig {
            max_message_size: Some(8 * 1024 * 1024),
            max_frame_size: Some(8 * 1024 * 1024),
            ..WebSocketConfig::default()
        };
        let connection: crate::DesktopConnectionHandle = if self.endpoint.native_binding().is_some()
        {
            let slot = transport.native_slot(&self.endpoint).await?;
            let mut cancelled = slot.cancellation();
            let handshake = async {
                let stream = transport
                    .native_stream(&self.endpoint, RuntimeStreamKind::Desktop)
                    .await?;
                // Preparation can await Kubernetes while the assignment changes.
                // Keep this exact stream and revalidate before sending credentials.
                self.current(transport).await?;
                let handshake =
                    tokio_tungstenite::client_async_with_config(request, stream, Some(config))
                        .await;
                self.current(transport).await?;
                handshake.map_err(handshake_failure)
            };
            let (socket, _) = tokio::select! { biased;
                _ = cancelled.wait_for(|cancelled| *cancelled) => return Err(refused()),
                result = handshake => result?,
            };
            Box::new(Connection { socket, control })
        } else {
            let handshake =
                tokio_tungstenite::connect_async_with_config(request, Some(config), true).await;
            self.current(transport).await?;
            let (socket, _) = handshake.map_err(handshake_failure)?;
            Box::new(Connection { socket, control })
        };
        Ok(connection)
    }
}

fn handshake_failure(error: tokio_tungstenite::tungstenite::Error) -> AdapterError {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = error {
        let mut states = response.headers().get_all(DESKTOP_STATE_HEADER).iter();
        if response.status() == http::StatusCode::SERVICE_UNAVAILABLE
            && states
                .next()
                .is_some_and(|state| state == DESKTOP_STATE_FAILED)
            && states.next().is_none()
        {
            return AdapterError::DesktopFailed;
        }
    }
    refused()
}

struct Connection<S> {
    socket: tokio_tungstenite::WebSocketStream<S>,
    control: Arc<dyn DesktopOwnershipControl>,
}

#[async_trait::async_trait]
impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static>
    crate::DesktopConnection for Connection<S>
{
    fn ownership_control(&self) -> Option<Arc<dyn DesktopOwnershipControl>> {
        Some(self.control.clone())
    }
    async fn recv(&mut self) -> crate::Result<Option<crate::DesktopFrame>> {
        let Some(frame) = self.socket.next().await else {
            return Ok(None);
        };
        Ok(Some(match frame.map_err(|_| refused())? {
            Message::Binary(bytes) => crate::DesktopFrame::Binary(bytes),
            Message::Text(text) => crate::DesktopFrame::Text(text),
            Message::Ping(bytes) => crate::DesktopFrame::Ping(bytes),
            Message::Pong(bytes) => crate::DesktopFrame::Pong(bytes),
            Message::Close(_) => crate::DesktopFrame::Close,
            _ => return Ok(None),
        }))
    }
    async fn send(&mut self, frame: crate::DesktopFrame) -> crate::Result<()> {
        let frame = match frame {
            crate::DesktopFrame::Binary(bytes) => Message::Binary(bytes),
            crate::DesktopFrame::Text(text) => Message::Text(text),
            crate::DesktopFrame::Ping(bytes) => Message::Ping(bytes),
            crate::DesktopFrame::Pong(bytes) => Message::Pong(bytes),
            crate::DesktopFrame::Close => Message::Close(None),
        };
        self.socket.send(frame).await.map_err(|_| refused())
    }
}
