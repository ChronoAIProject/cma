use super::*;
use futures_util::StreamExt as _;

const CAPACITY_RECOVERY_PROOF_BYTES: usize = 256 * 1024;

#[async_trait::async_trait]
impl super::super::HostedControlTransport for super::ProviderHostedControlTransport {
    async fn reattach_existing_capacity_recovery(
        &self,
        request: &crate::agent_capacity_recovery::CapacityRecoveryRequest,
        updates: tokio::sync::mpsc::Sender<crate::ConversationUpdate>,
    ) -> Result<
        (
            crate::ConversationChannel,
            crate::agent_capacity_recovery::CapacityRecoveryConfirmation,
        ),
        crate::agent_capacity_recovery::CapacityRecoveryError,
    > {
        request
            .validate()
            .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::IdentityConflict)?;
        // Close/retire during any discovery or attachment await must invalidate
        // this attempt before it can replace a newer registered conversation.
        let key = (
            request.target.runtime.clone(),
            crate::AgentSlot::new(&request.target.agent_id),
        );
        let epochs = self.open_epochs(&key).await;
        let endpoint = self
            .endpoint(&request.target.runtime)
            .await
            .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable)?;
        let url = endpoint
            .capacity_recovery(&request.target.agent_id)
            .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable)?;
        let (header_name, token) = self.auth_header(&endpoint);
        let response = self
            .http
            .post(url)
            .header(header_name, token)
            .json(request)
            .send()
            .await
            .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable)?;
        let status = response.status();
        if !status.is_success() {
            return Err(match status {
                reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::NOT_IMPLEMENTED => {
                    crate::agent_capacity_recovery::CapacityRecoveryError::Unsupported
                }
                reqwest::StatusCode::CONFLICT | reqwest::StatusCode::BAD_REQUEST => {
                    crate::agent_capacity_recovery::CapacityRecoveryError::IdentityConflict
                }
                _ => crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable,
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > CAPACITY_RECOVERY_PROOF_BYTES as u64)
        {
            return Err(crate::agent_capacity_recovery::CapacityRecoveryError::StateUnconfirmed);
        }
        let mut body = response.bytes_stream();
        let mut proof_bytes = Vec::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk
                .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable)?;
            if chunk.len() > CAPACITY_RECOVERY_PROOF_BYTES.saturating_sub(proof_bytes.len()) {
                return Err(
                    crate::agent_capacity_recovery::CapacityRecoveryError::StateUnconfirmed,
                );
            }
            proof_bytes.extend_from_slice(&chunk);
        }
        let proof = serde_json::from_slice::<
            crate::agent_capacity_recovery::CapacityRecoveryNativeProof,
        >(&proof_bytes)
        .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::StateUnconfirmed)?;
        proof
            .confirmation
            .validate_for(request)
            .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::StateUnconfirmed)?;
        let recovered_prompt = match &proof.confirmation.native_state {
            crate::agent_capacity_recovery::ConfirmedAgentState::Idle => None,
            crate::agent_capacity_recovery::ConfirmedAgentState::Working { prompt_id }
            | crate::agent_capacity_recovery::ConfirmedAgentState::WaitingApproval {
                prompt_id,
                ..
            } => crate::PromptIdentity::parse(prompt_id),
        };
        if request.target.active_turn.is_some() && recovered_prompt.is_none() {
            return Err(crate::agent_capacity_recovery::CapacityRecoveryError::StateUnconfirmed);
        }
        let socket_url = endpoint
            .conversation_for_capacity_recovery(&request.target.agent_id, &request.challenge)
            .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable)?;
        let socket = self
            .connect_url(
                &endpoint,
                &crate::AgentSlot::new(&request.target.agent_id),
                socket_url,
                None,
            )
            .await
            .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable)?;
        let settings = super::HostedAgentOpenSettings {
            coordinated_ingress: true,
            retained_json: true,
            bounded_raw_output: true,
            bounded_permission_state: true,
            output_json_budget: true,
            shared_runtime_output: true,
            already_initialized_error: Some(serde_json::json!({
                "code": -32603,
                "message": "Internal error",
                "data": { "details": "Already initialized" },
            })),
            open_completion: None,
            context_refresh: None,
            permission_mode: None,
            own_credential: false,
            channel_tool: None,
            provider_tool: None,
            access_tool: None,
            recovered_prompt,
            recovered_context_sequence: None,
        };
        let opened = Bridge::open_with_relay_policy(
            socket,
            &request.target.agent_id,
            proof.cwd,
            (settings, None, true),
            Some(request.target.conversation_id.clone()),
            match &proof.confirmation.native_state {
                crate::agent_capacity_recovery::ConfirmedAgentState::Idle => None,
                crate::agent_capacity_recovery::ConfirmedAgentState::Working { prompt_id }
                | crate::agent_capacity_recovery::ConfirmedAgentState::WaitingApproval {
                    prompt_id,
                    ..
                } => crate::PromptIdentity::parse(prompt_id),
            },
            updates,
        )
        .await
        .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable)?;
        if opened.channel.conversation_id.as_deref()
            != Some(request.target.conversation_id.as_str())
        {
            opened.bridge.retire().await;
            opened.reader.abort();
            opened.relay.abort();
            return Err(crate::agent_capacity_recovery::CapacityRecoveryError::IdentityConflict);
        }
        self.register_link(key, epochs, opened)
            .await
            .map_err(|_| crate::agent_capacity_recovery::CapacityRecoveryError::Unavailable)
            .map(|channel| (channel, proof.confirmation))
    }

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
        self.pin_output_policy(&mut settings);
        let recovered_prompt = settings.recovered_prompt.take();
        let opened = Bridge::open_with_relay_policy(
            socket,
            launch.request.agent_id.as_str(),
            launch.request.cwd.clone(),
            (settings, subscription, self.bounded_update_relay),
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
