use super::*;

impl ProviderHostedControlTransport {
    pub fn new(
        client: SandboxClient,
        archive_factory: Arc<dyn CheckpointArchiveStoreFactory>,
    ) -> crate::Result<Self> {
        RuntimeEndpoint::parse(client.base_url(), "runtime.invalid")?;
        Self::with_discovery(RuntimeDiscovery::Provider(client), Some(archive_factory))
    }

    pub fn for_agent_sandbox(
        client: crate::agent_sandbox::AgentSandboxClient,
    ) -> crate::Result<Self> {
        Self::with_discovery(RuntimeDiscovery::AgentSandbox(client), None)
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
        Ok(Self {
            client,
            http,
            archive_factory,
            state: Arc::new(Mutex::new(TransportState::default())),
            subscriptions: None,
        })
    }

    pub(super) fn require_supported_operation(&self, request: &RequestFrame) -> crate::Result<()> {
        if self.archive_factory.is_none()
            && matches!(
                request,
                RequestFrame::Checkpoint(_)
                    | RequestFrame::CheckpointForTeardown(_)
                    | RequestFrame::ResumeCheckpointWriters(_)
                    | RequestFrame::BootstrapRestore(_)
                    | RequestFrame::Restore(_)
            )
        {
            return Err(AdapterError::Unsupported(
                "agent-sandbox persists its home on the attached volume; CMA archives are retired"
                    .into(),
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
                let binding = client.pod_binding(handle).await?;
                let address = std::net::SocketAddr::new(binding.pod_ip, SUPERVISOR_PORT);
                RuntimeEndpoint::parse("http://runtime.invalid", &format!("http://{address}"))
                    .map(|e| e.with_control_token(client.control_token(&binding.sandbox_name)))
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
