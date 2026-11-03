//! Tools admitted from durable provider-binding provenance, independently of channel bridges.
use crate::{AdapterError, AgentOpenRequest, RuntimeHandle, ToolProxyRequest, ToolProxyStatus};

mod transport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedToolOpenReceipt {
    pub boot_id: String,
    pub conversation: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct HostedProviderToolBinding {
    provenance_sha256: String,
    runtime: RuntimeHandle,
    request: AgentOpenRequest,
    authority_fence: u64,
    proxy: ToolProxyRequest,
    retained: Option<HostedToolOpenReceipt>,
    observed: Option<ToolProxyStatus>,
    relay: Option<transport::Relay>,
}

impl std::fmt::Debug for HostedProviderToolBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostedProviderToolBinding(<protected>)")
    }
}

impl HostedProviderToolBinding {
    /// Only a trusted source may construct this after checking exact binding,
    /// conversation reservation, Agent creation, profile and runtime authority.
    pub fn new(
        provenance_sha256: String,
        runtime: RuntimeHandle,
        request: AgentOpenRequest,
        authority_fence: u64,
        proxy: ToolProxyRequest,
        retained: Option<HostedToolOpenReceipt>,
    ) -> crate::Result<Self> {
        proxy.validate()?;
        if provenance_sha256.len() != 64
            || !provenance_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || authority_fence == 0
            || !request.has_valid_sha256()
            || retained.as_ref().is_some_and(|value| {
                !valid_conversation(&value.conversation)
                    || ToolProxyStatus {
                        request: proxy.clone(),
                        boot_id: value.boot_id.clone(),
                    }
                    .validate_for(&proxy)
                    .is_err()
            })
        {
            return Err(invalid());
        }
        Ok(Self {
            provenance_sha256,
            runtime,
            request,
            authority_fence,
            proxy,
            retained,
            observed: None,
            relay: None,
        })
    }

    pub fn scope(&self) -> (&RuntimeHandle, &AgentOpenRequest, u64, &ToolProxyRequest) {
        (
            &self.runtime,
            &self.request,
            self.authority_fence,
            &self.proxy,
        )
    }

    pub fn observed(&self) -> Option<&ToolProxyStatus> {
        self.observed.as_ref()
    }

    pub fn provenance_sha256(&self) -> &str {
        &self.provenance_sha256
    }

    /// Correlate the supervisor response before opening or confirming tools.
    /// The hosted adapter always obtains and replaces this observation itself.
    pub fn observe(&mut self, status: ToolProxyStatus) -> crate::Result<()> {
        status.validate_for(&self.proxy)?;
        self.observed = Some(status);
        Ok(())
    }

    pub(super) fn retained_for(&self, conversation: &str) -> bool {
        self.retained.as_ref().is_some_and(|receipt| {
            receipt.conversation == conversation
                && self
                    .observed
                    .as_ref()
                    .is_some_and(|status| status.boot_id == receipt.boot_id)
        })
    }

    pub(super) fn acp_server(&self) -> serde_json::Value {
        use agent_client_protocol_schema::v1::{HttpHeader, McpServer, McpServerHttp};
        let server = match &self.relay {
            Some(relay) => {
                McpServerHttp::new("cmaeg", &relay.endpoint).headers(vec![HttpHeader::new(
                    "Authorization",
                    format!("Bearer {}", relay.credential.expose()),
                )])
            }
            None => McpServerHttp::new("cmaeg", crate::LOCAL_TOOL_PROXY_URL),
        };
        serde_json::to_value(McpServer::Http(server)).expect("typed MCP descriptor serializes")
    }
}

#[async_trait::async_trait]
pub trait HostedProviderToolSource: Send + Sync {
    async fn resolve(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        authority_fence: u64,
    ) -> crate::Result<Option<HostedProviderToolBinding>>;

    /// Called only after the runtime observation, handshake and exact launch
    /// receipt pass. Recheck current authority before retaining this proof.
    async fn confirm_open(
        &self,
        binding: &HostedProviderToolBinding,
        conversation: &str,
    ) -> crate::Result<()>;

    /// Only a durable, fenced idle maintenance intent may change transport.
    async fn prepare_transport(
        &self,
        _binding: &HostedProviderToolBinding,
        _origin: &str,
        _conversation: &str,
        _operation_sequence: u64,
    ) -> crate::Result<HostedProviderToolBinding> {
        Err(AdapterError::Unsupported(
            "provider tool transport migration is unavailable".into(),
        ))
    }

    /// Persist installation only after the same native conversation reloads.
    async fn confirm_transport(
        &self,
        _binding: &HostedProviderToolBinding,
        _conversation: &str,
        _operation_sequence: u64,
    ) -> crate::Result<()> {
        Err(AdapterError::Unsupported(
            "provider tool transport migration is unavailable".into(),
        ))
    }
}

fn valid_conversation(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("provider tool attachment authority is invalid".into())
}
