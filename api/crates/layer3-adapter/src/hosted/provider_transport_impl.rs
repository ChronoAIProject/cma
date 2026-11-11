use super::*;

#[async_trait::async_trait]
impl super::super::HostedControlTransport for super::ProviderHostedControlTransport {
    async fn open_desktop(
        &self,
        handle: &RuntimeHandle,
    ) -> crate::Result<crate::DesktopConnectionHandle> {
        let endpoint = self.endpoint(handle).await?;
        let stream = self
            .native_stream(&endpoint, RuntimeStreamKind::Desktop)
            .await?;
        let mut request = endpoint
            .desktop()?
            .as_str()
            .into_client_request()
            .map_err(|_| AdapterError::Rejected("runtime desktop request is invalid".into()))?;
        let (name, token) = self.auth_header(&endpoint);
        let mut header = http::HeaderValue::from_str(token)
            .map_err(|_| AdapterError::Rejected("runtime token is not a valid header".into()))?;
        header.set_sensitive(true);
        request.headers_mut().insert(name, header);
        let config = WebSocketConfig {
            max_message_size: Some(8 * 1024 * 1024),
            max_frame_size: Some(8 * 1024 * 1024),
            ..WebSocketConfig::default()
        };
        let (socket, _) = tokio::time::timeout(
            CONNECT_DEADLINE,
            tokio_tungstenite::client_async_with_config(request, stream, Some(config)),
        )
        .await
        .map_err(|_| AdapterError::Unavailable("runtime desktop connection timed out".into()))?
        .map_err(|_| AdapterError::Unavailable("runtime desktop WebSocket failed".into()))?;
        Ok(Box::new(TungsteniteDesktopConnection { socket }))
    }

    async fn inspect_native_status(
        &self,
        inspection: &super::super::NativeStatusInspection<'_>,
    ) -> crate::Result<Option<crate::RuntimeInspection>> {
        self.inspect_reused_native_status(inspection).await
    }

    async fn bind_runtime(&self, handle: &RuntimeHandle, sandbox_id: &str) -> crate::Result<()> {
        self.bind_sandbox(handle, sandbox_id).await
    }

    async fn bind_native_runtime(
        &self,
        handle: &RuntimeHandle,
        sandbox_id: &str,
        request_sha256: &str,
    ) -> crate::Result<()> {
        self.bind_native_identity(handle, sandbox_id, request_sha256)
            .await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        self.require_supported_operation(&request)?;
        #[cfg(test)]
        let status_request = matches!(&request, RequestFrame::RuntimeStatus(_));
        #[cfg(test)]
        if status_request {
            Self::test_status_boundary(false);
        }
        let endpoint = self.endpoint(handle).await?;
        let response = self.exchange_at(handle, &endpoint, request).await;
        #[cfg(test)]
        if status_request && response.is_ok() {
            Self::test_status_boundary(true);
        }
        response
    }

    async fn verify_checkpoint_hold(
        &self,
        handle: &RuntimeHandle,
        request: &crate::CheckpointHoldRequest,
    ) -> crate::Result<ResponseFrame> {
        // Require the sandbox binding, but do not transfer the archive again.
        // The caller already published it and checks the pinned identity.
        self.archive(handle).await?;
        let endpoint = self.endpoint(handle).await?;
        self.control_with_hold_recovery(
            &endpoint,
            &RequestFrame::CheckpointForTeardown(request.clone()),
        )
        .await
    }

    async fn export_agent_history(
        &self,
        handle: &RuntimeHandle,
        agent: &AgentSlot,
    ) -> crate::Result<Vec<u8>> {
        ProviderHostedControlTransport::export_agent_history(self, handle, agent).await
    }

    #[tracing::instrument(skip_all, fields(agent_id = launch.request.agent_id.as_str()))]
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        mut settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        let mut timing = super::super::startup_timing::StartupTiming::new();
        let key = (handle.clone(), launch.request.agent_id.clone());
        let epochs = self.open_epochs(&key).await;
        let endpoint = self.endpoint(handle).await?;
        timing.completed("transport_endpoint");
        let (mut receipt, socket) = self
            .open_and_connect(handle, &endpoint, &launch, &mut timing)
            .await?;
        timing.completed("conversation_transport");
        let subscription = if settings.own_credential {
            match &self.subscriptions {
                Some(source) => Some(super::super::subscriptions::HostedSubscriptionBinding {
                    source: source.clone(),
                    sandbox_id: self.binding(handle).await?.sandbox_id,
                    runtime: handle.clone(),
                    agent: launch.request.agent_id.clone(),
                }),
                None => None,
            }
        } else {
            None
        };
        settings.bounded_permission_state = self.bounded_permission_state;
        let recovered_prompt = settings.recovered_prompt.take();
        let opened = Bridge::open_with_subscription_and_prompt(
            socket,
            launch.request.agent_id.as_str(),
            launch.request.cwd.clone(),
            (settings, subscription),
            resume_conversation,
            recovered_prompt,
            updates,
        )
        .await?;
        timing.completed("conversation_handshake");
        let channel = self.register_link(key, epochs, opened).await?;
        receipt.receipt.conversation_id = channel.conversation_id.clone();
        Ok((channel, receipt))
    }

    async fn retire(&self, handle: &RuntimeHandle) -> crate::Result<()> {
        let links = {
            let mut state = self.state.lock().await;
            let epoch = state.runtime_epochs.entry(handle.clone()).or_default();
            *epoch = epoch.wrapping_add(1);
            state.bindings.remove(handle);
            if let Some(slot) = state.native_http.remove(handle) {
                slot.cancel();
            }
            state
                .slot_epochs
                .retain(|(runtime, _), _| runtime != handle);
            let keys: Vec<_> = state
                .links
                .keys()
                .filter(|(runtime, _)| runtime == handle)
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| state.links.remove(&key))
                .collect::<Vec<_>>()
        };
        for link in links {
            link.stop().await;
        }
        Ok(())
    }
}

struct TungsteniteDesktopConnection {
    socket: tokio_tungstenite::WebSocketStream<crate::agent_sandbox::RuntimeStream>,
}

#[async_trait::async_trait]
impl crate::DesktopConnection for TungsteniteDesktopConnection {
    async fn recv(&mut self) -> crate::Result<Option<crate::DesktopFrame>> {
        use futures_util::StreamExt as _;
        let Some(message) = self.socket.next().await else {
            return Ok(None);
        };
        let message = message
            .map_err(|_| AdapterError::Unavailable("runtime desktop stream closed".into()))?;
        Ok(Some(match message {
            tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                crate::DesktopFrame::Binary(bytes.to_vec())
            }
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                crate::DesktopFrame::Text(text.to_string())
            }
            tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
                crate::DesktopFrame::Ping(bytes.to_vec())
            }
            tokio_tungstenite::tungstenite::Message::Pong(bytes) => {
                crate::DesktopFrame::Pong(bytes.to_vec())
            }
            tokio_tungstenite::tungstenite::Message::Close(_) => crate::DesktopFrame::Close,
            _ => return Ok(None),
        }))
    }

    async fn send(&mut self, frame: crate::DesktopFrame) -> crate::Result<()> {
        use futures_util::SinkExt as _;
        let message = match frame {
            crate::DesktopFrame::Binary(bytes) => {
                tokio_tungstenite::tungstenite::Message::Binary(bytes)
            }
            crate::DesktopFrame::Text(text) => tokio_tungstenite::tungstenite::Message::Text(text),
            crate::DesktopFrame::Ping(bytes) => {
                tokio_tungstenite::tungstenite::Message::Ping(bytes)
            }
            crate::DesktopFrame::Pong(bytes) => {
                tokio_tungstenite::tungstenite::Message::Pong(bytes)
            }
            crate::DesktopFrame::Close => tokio_tungstenite::tungstenite::Message::Close(None),
        };
        self.socket
            .send(message)
            .await
            .map_err(|_| AdapterError::Unavailable("runtime desktop stream closed".into()))
    }
}
