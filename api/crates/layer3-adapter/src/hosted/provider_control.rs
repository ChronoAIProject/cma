use super::*;

impl ProviderHostedControlTransport {
    pub fn with_native_close_agent_lane(mut self, enabled: bool) -> Self {
        self.native_close_agent_lane = enabled;
        self
    }

    pub(super) async fn control(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        let _ownership = self.receiver_lease(endpoint).await?;
        self.control_exchange(endpoint, request).await
    }

    pub(super) async fn open_on_receiver_lease(
        &self,
        endpoint: &RuntimeEndpoint,
        launch: &FencedRequest<AgentLaunch>,
        ownership: &ownership::ReceiverLease,
    ) -> crate::Result<ResponseFrame> {
        let request = RequestFrame::OpenAgent(launch.clone());
        self.require_supported_operation(&request)?;
        ownership
            .check_open(self, endpoint, &launch.request.agent_id)
            .await?;
        tracing::debug!(
            stage = "open_receiver_lease_reuse",
            "Open reuses its live receiver lease"
        );
        self.control_exchange(endpoint, &request).await
    }

    fn control_exchange<'a>(
        &'a self,
        endpoint: &'a RuntimeEndpoint,
        request: &'a RequestFrame,
    ) -> futures_util::future::BoxFuture<'a, crate::Result<ResponseFrame>> {
        // The native HTTP future is large. Sharing the exchange must not inline
        // another copy of its state into each caller's ownership/capacity scope.
        Box::pin(async move {
            let url = endpoint.control()?;
            let auth = self.auth_header(endpoint);
            let reads_status = matches!(request, RequestFrame::RuntimeStatus(_));
            let exchange = async {
                if endpoint.native_binding().is_some() {
                    let lane = if self.native_close_agent_lane
                        && matches!(request, RequestFrame::CloseAgent(_))
                        && crate::owner_protocol::current_authority().is_some()
                    {
                        native_pool::NativeHttpLane::CloseAgent
                    } else {
                        native_pool::NativeHttpLane::Control
                    };
                    let body = serde_json::to_vec(request).map_err(|_| {
                        AdapterError::Rejected("runtime request could not be encoded".into())
                    })?;
                    let reply = self
                        .native_http_for(
                            endpoint,
                            http::Method::POST,
                            url,
                            body,
                            super::super::provider_http::CONTROL_RESPONSE_BYTES,
                            (lane, request_capacity_class(request)),
                        )
                        .await?;
                    if reads_status {
                        crate::receiver_failure::record_status(reply.status);
                    }
                    return self.complete_native_control(endpoint, request, reply).await;
                }
                let mut request_builder = self
                    .http
                    .post(url)
                    .header(auth.0, auth.1)
                    .header(
                        reqwest::header::ACCEPT,
                        format!(
                            "{}, {}, {}",
                            crate::wire::AGENT_RESOURCES_STATUS_MEDIA_TYPE,
                            crate::wire::AGENT_COMPUTER_STATUS_MEDIA_TYPE,
                            crate::wire::AGENT_SKILL_UPGRADE_STATUS_MEDIA_TYPE,
                        ),
                    )
                    .json(request);
                if let Some(owner) = crate::owner_protocol::current_header()? {
                    request_builder =
                        request_builder.header(crate::owner_protocol::OWNER_HEADER, owner);
                }
                let response = request_builder.send().await.map_err(|error| {
                    AdapterError::Unavailable(format!("runtime control request failed: {error}"))
                })?;
                if reads_status {
                    crate::receiver_failure::record_status(response.status());
                }
                super::super::provider_http::decode_control(response).await
            };
            let response = crate::file_read_diagnostics::transport(
                request.operation(),
                crate::file_read_diagnostics::TransportPhase::Control,
                async {
                    tokio::time::timeout(CONTROL_DEADLINE, exchange)
                        .await
                        .map_err(|_| {
                            AdapterError::Unavailable("runtime control request timed out".into())
                        })?
                },
            )
            .await?;
            control_correlation::validate(request, &response)?;
            if let Some(authority) = crate::owner_protocol::current_authority() {
                authority.check().await?;
            }
            Ok(response)
        })
    }

    pub(super) async fn status_on_native_lease(
        &self,
        endpoint: &RuntimeEndpoint,
        status: &crate::RuntimeStatusRequest,
        lease: native_pool::NativeLease,
    ) -> crate::Result<ResponseFrame> {
        let request = RequestFrame::RuntimeStatus(status.clone());
        let body = serde_json::to_vec(&request)
            .map_err(|_| AdapterError::Rejected("runtime request could not be encoded".into()))?;
        let reply = self
            .native_http_once(
                endpoint,
                http::Method::POST,
                endpoint.control()?,
                body,
                super::super::provider_http::CONTROL_RESPONSE_BYTES,
                lease,
            )
            .await?;
        crate::receiver_failure::record_status(reply.status);
        self.complete_native_control(endpoint, &request, reply)
            .await
    }

    async fn complete_native_control(
        &self,
        endpoint: &RuntimeEndpoint,
        request: &RequestFrame,
        reply: native_transport::NativeHttpResponse,
    ) -> crate::Result<ResponseFrame> {
        let mut response =
            super::super::provider_http::decode_control_bytes(reply.status, &reply.bytes)?;
        let (_, token) = self.auth_header(endpoint);
        if let ResponseFrame::Failure(failure) = &mut response {
            if !token.is_empty() {
                failure.code = failure.code.replace(token, "[redacted]");
                failure.detail = failure.detail.replace(token, "[redacted]");
            }
        }
        control_correlation::validate(request, &response)?;
        reply.recycle().await?;
        Ok(response)
    }
}

// Exhaustive classification prevents a new operation from silently borrowing reserve.
fn request_capacity_class(request: &RequestFrame) -> RuntimeWorkloadClass {
    match request {
        RequestFrame::RuntimeStatus(_)
        | RequestFrame::Resources
        | RequestFrame::ToolProxyStatus(_)
        | RequestFrame::WorkspaceRemovalSupport(_)
        | RequestFrame::ListAgentFiles(_)
        | RequestFrame::ReadAgentFile(_)
        | RequestFrame::CaptureAgent(_)
        | RequestFrame::OpenAgent(_)
        | RequestFrame::BrowserControl(_)
        | RequestFrame::Browser(_)
        | RequestFrame::UploadAgentAttachment(_)
        | RequestFrame::WriteAgentFile(_)
        | RequestFrame::DeleteAgentFile(_) => RuntimeWorkloadClass::Ordinary,
        RequestFrame::CacheSkillArchive(_)
        | RequestFrame::ReconcileWorkspaceRemoval(_)
        | RequestFrame::Install(_)
        | RequestFrame::BootstrapRestore(_)
        | RequestFrame::PrepareWorkspace(_)
        | RequestFrame::RecoverWorkspacePrepare(_)
        | RequestFrame::WorkspaceSetup(_)
        | RequestFrame::RemoveWorkspace(_)
        | RequestFrame::PrepareAgentResources(_)
        | RequestFrame::AddWorktree(_)
        | RequestFrame::RemoveWorktree(_)
        | RequestFrame::MaterializeAgentHome(_)
        | RequestFrame::InstallAgentSkill(_)
        | RequestFrame::RemoveAgentHome(_)
        | RequestFrame::CloseAgent(_)
        | RequestFrame::Checkpoint(_)
        | RequestFrame::CheckpointForTeardown(_)
        | RequestFrame::ResumeCheckpointWriters(_)
        | RequestFrame::Restore(_)
        | RequestFrame::Credential(_) => RuntimeWorkloadClass::Control,
    }
}
