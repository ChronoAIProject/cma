use super::*;

impl ProviderHostedControlTransport {
    /// Legacy archive protocol for callers that explicitly supply storage authority.
    /// CMA production uses the archive-free constructors below.
    pub fn new(
        client: SandboxClient,
        archive_factory: Arc<dyn CheckpointArchiveStoreFactory>,
    ) -> crate::Result<Self> {
        RuntimeEndpoint::parse(client.base_url(), "runtime.invalid")?;
        Self::with_discovery(
            RuntimeDiscovery::Provider(Arc::new(client)),
            Some(archive_factory),
        )
    }

    /// Retained runtimes remain addressable, including release of preexisting writer holds.
    /// No new checkpoint, restore, or archive transfer is permitted.
    pub fn for_retained_opensandbox(client: SandboxClient) -> crate::Result<Self> {
        RuntimeEndpoint::parse(client.base_url(), "runtime.invalid")?;
        Self::with_discovery(RuntimeDiscovery::Provider(Arc::new(client)), None)
    }

    pub fn for_agent_sandbox(
        client: crate::agent_sandbox::AgentSandboxClient,
    ) -> crate::Result<Self> {
        Self::for_agent_sandbox_with_transport(client, NativeRuntimeTransport::Direct)
    }

    pub fn for_agent_sandbox_with_transport(
        client: crate::agent_sandbox::AgentSandboxClient,
        mode: NativeRuntimeTransport,
    ) -> crate::Result<Self> {
        let mut transport =
            Self::with_discovery(RuntimeDiscovery::AgentSandbox(Arc::new(client)), None)?;
        transport.native_transport = mode;
        Ok(transport)
    }

    fn with_discovery(
        client: RuntimeDiscovery,
        archive_factory: Option<Arc<dyn CheckpointArchiveStoreFactory>>,
    ) -> crate::Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_DEADLINE)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                AdapterError::Unavailable(format!(
                    "runtime HTTP client could not be built: {error}"
                ))
            })?;
        let runtime_capacity = match &client {
            RuntimeDiscovery::AgentSandbox(client) => client.runtime_capacity(),
            RuntimeDiscovery::Provider(_) => {
                RuntimeCapacityAuthority::new(Default::default()).map_err(AdapterError::Rejected)?
            }
        };
        Ok(Self {
            desktop_http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
                .map_err(|_| {
                    AdapterError::Unavailable("desktop control client could not be built".into())
                })?,
            owner_router: None,
            output_json_budget: false,
            client,
            http,
            archive_factory,
            state: Arc::new(Mutex::new(TransportState::default())),
            subscriptions: None,
            coordinated_ingress: false,
            retained_json: false,
            bounded_raw_output: false,
            shared_runtime_output: false,
            native_close_agent_lane: false,
            open_reply_loss: None,
            native_transport: NativeRuntimeTransport::Direct,
            legacy_receiver_image: None,
            stale_turn_image: None,
            #[cfg(test)]
            native_http_budget: runtime_capacity.requests.clone(),
            runtime_capacity,
            bounded_permission_state: false,
            bounded_update_relay: false,
        })
    }

    pub(super) fn require_supported_operation(&self, request: &RequestFrame) -> crate::Result<()> {
        let native_writer_resume = matches!(self.client, RuntimeDiscovery::AgentSandbox(_))
            && matches!(request, RequestFrame::ResumeCheckpointWriters(_));
        if self.archive_factory.is_none()
            && (native_writer_resume
                || matches!(
                    request,
                    RequestFrame::Checkpoint(_)
                        | RequestFrame::CheckpointForTeardown(_)
                        | RequestFrame::BootstrapRestore(_)
                        | RequestFrame::Restore(_)
                ))
        {
            return Err(AdapterError::Unsupported(
                "CMA archives are retired; Sandbox state belongs to its persistent volume".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn endpoint(&self, handle: &RuntimeHandle) -> crate::Result<RuntimeEndpoint> {
        match &self.client {
            RuntimeDiscovery::Provider(client) => {
                let advertised = client.endpoint(handle.as_str(), SUPERVISOR_PORT).await?;
                RuntimeEndpoint::parse(client.base_url(), &advertised.endpoint)
            }
            RuntimeDiscovery::AgentSandbox(client) => {
                let (identity, cached) = self.native_endpoint_state(handle).await;
                let binding = match cached {
                    Some(binding) if self.native_transport != NativeRuntimeTransport::Direct => {
                        binding
                    }
                    _ => client.pod_binding(handle).await?,
                };
                let address = std::net::SocketAddr::new(binding.pod_ip, SUPERVISOR_PORT);
                let endpoint =
                    RuntimeEndpoint::parse("http://runtime.invalid", &format!("http://{address}"))?
                        .with_control_token(client.control_token(&binding.sandbox_name));
                Ok(if self.native_transport != NativeRuntimeTransport::Direct {
                    endpoint
                        .with_native_binding(handle.clone(), binding)
                        .with_native_identity(identity)
                } else {
                    endpoint.with_direct_receiver(handle, &binding)
                })
            }
        }
    }

    pub(super) fn auth_header<'a>(
        &'a self,
        endpoint: &'a RuntimeEndpoint,
    ) -> (&'static str, &'a str) {
        match &self.client {
            RuntimeDiscovery::Provider(client) => client.auth_header(),
            RuntimeDiscovery::AgentSandbox(_) => (
                "x-cma-runtime-token",
                endpoint.control_token().unwrap_or_default(),
            ),
        }
    }
}
