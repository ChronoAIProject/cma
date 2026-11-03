//! A control-plane tool connection, kept outside retained launch/home payloads.
//! The source must prove the exact owned Agent and runtime before returning a binding.

use crate::{AdapterError, AgentOpenRequest, RuntimeCredential, RuntimeHandle};

#[derive(Clone, PartialEq, Eq)]
pub struct HostedAccessToolBinding {
    endpoint: String,
    credential: RuntimeCredential,
    runtime: RuntimeHandle,
    agent: crate::AgentSlot,
    request_sha256: String,
    authority_fence: u64,
    retained_conversation: Option<String>,
    prepared_conversation: Option<String>,
}

impl std::fmt::Debug for HostedAccessToolBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostedAccessToolBinding(<protected>)")
    }
}

impl HostedAccessToolBinding {
    /// Safe exact scope for the source's durable successful-open receipt.
    pub fn scope(&self) -> (&RuntimeHandle, &crate::AgentSlot, &str, u64) {
        (
            &self.runtime,
            &self.agent,
            &self.request_sha256,
            self.authority_fence,
        )
    }

    /// Trusted source only: the bearer is a CMA tool capability, never a NyxID
    /// key. It must resolve to this exact Agent creation and runtime incarnation.
    /// `retained_conversation` requires a durable successful tool-open receipt
    /// for that same incarnation; the requested resume ID alone is insufficient.
    pub fn new(
        control_plane_origin: &str,
        credential: RuntimeCredential,
        runtime: RuntimeHandle,
        request: &AgentOpenRequest,
        authority_fence: u64,
        retained_conversation: Option<String>,
    ) -> crate::Result<Self> {
        let mut endpoint = url::Url::parse(control_plane_origin).map_err(|_| invalid())?;
        if endpoint.scheme() != "https"
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !matches!(endpoint.path(), "" | "/")
            || !valid_tool_credential(credential.expose())
            || authority_fence == 0
            || !request.has_valid_sha256()
            || retained_conversation.as_deref().is_some_and(|id| {
                id.is_empty() || id.len() > 512 || id.chars().any(char::is_control)
            })
        {
            return Err(invalid());
        }
        endpoint.set_path("/api/v1/agent-tools");
        Ok(Self {
            endpoint: endpoint.to_string(),
            credential,
            runtime,
            agent: request.options.slot.clone(),
            request_sha256: request.request_sha256.clone(),
            authority_fence,
            retained_conversation,
            prepared_conversation: None,
        })
    }

    /// Trusted source only: an exclusively claimed, never-prompted prepared
    /// conversation may install its first owner tool binding. The source must
    /// prove the durable preparation/adoption receipt, exact runtime boot,
    /// request and conversation, and absence of any admitted owner turn.
    /// This is not authority to reconfigure an existing owner's conversation.
    pub fn with_prepared_conversation(mut self, conversation: &str) -> crate::Result<Self> {
        if self.retained_conversation.is_some()
            || conversation.is_empty()
            || conversation.len() > 512
            || conversation.chars().any(char::is_control)
        {
            return Err(invalid());
        }
        self.prepared_conversation = Some(conversation.into());
        Ok(self)
    }

    pub(super) fn require_request(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        authority_fence: u64,
    ) -> crate::Result<()> {
        if &self.runtime != runtime
            || self.agent != request.options.slot
            || self.request_sha256 != request.request_sha256
            || self.authority_fence != authority_fence
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub(super) fn retained_for(&self, conversation: &str) -> bool {
        self.retained_conversation.as_deref() == Some(conversation)
    }

    pub(super) fn prepared_for(&self, conversation: &str) -> bool {
        self.prepared_conversation.as_deref() == Some(conversation)
    }

    pub(super) fn acp_server(&self) -> serde_json::Value {
        use agent_client_protocol_schema::v1::{HttpHeader, McpServer, McpServerHttp};
        let server = McpServer::Http(McpServerHttp::new("cma_access", &self.endpoint).headers(
            vec![HttpHeader::new(
                "Authorization",
                format!("Bearer {}", self.credential.expose()),
            )],
        ));
        serde_json::to_value(server).expect("typed MCP HTTP descriptor serializes")
    }
}

/// Resolve an access-request capability from current Agent/runtime authority.
/// This credential can request owner consent; it cannot change any service grant.
#[async_trait::async_trait]
pub trait HostedAccessToolSource: Send + Sync {
    async fn resolve(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        authority_fence: u64,
    ) -> crate::Result<Option<HostedAccessToolBinding>>;

    /// Persist only after the actual handshake and exact launch receipt pass.
    /// A failed/unknown open cannot be recorded as installed tool configuration.
    async fn confirm_open(
        &self,
        binding: &HostedAccessToolBinding,
        conversation: &str,
    ) -> crate::Result<()>;

    /// Resolve only a durable, fenced idle maintenance intent. A requested URL
    /// alone never grants authority to replace a retained tool connection.
    async fn prepare_transport(
        &self,
        _binding: &HostedAccessToolBinding,
        _origin: &str,
        _conversation: &str,
        _operation_sequence: u64,
    ) -> crate::Result<HostedAccessToolBinding> {
        Err(AdapterError::Unsupported(
            "retained tool transport migration is unavailable".into(),
        ))
    }

    /// Confirm only after the same native conversation acknowledged its new
    /// tools and retained settings. Unknown results replay the durable intent.
    async fn confirm_transport(
        &self,
        _binding: &HostedAccessToolBinding,
        _conversation: &str,
        _operation_sequence: u64,
    ) -> crate::Result<()> {
        Err(AdapterError::Unsupported(
            "retained tool transport migration is unavailable".into(),
        ))
    }
}

fn valid_tool_credential(value: &str) -> bool {
    value.strip_prefix("cma_at_").is_some_and(|suffix| {
        suffix.len() == 64
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("access tool connection binding is invalid".into())
}
