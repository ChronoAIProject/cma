//! Tools admitted from durable provider-binding provenance, independently of channel bridges.
use crate::{AdapterError, AgentOpenRequest, RuntimeHandle, ToolProxyRequest, ToolProxyStatus};

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
        use agent_client_protocol_schema::v1::{McpServer, McpServerHttp};
        serde_json::to_value(McpServer::Http(McpServerHttp::new(
            "cmaeg",
            crate::LOCAL_TOOL_PROXY_URL,
        )))
        .expect("typed MCP descriptor serializes")
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
