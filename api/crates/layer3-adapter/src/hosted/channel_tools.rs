//! A control-plane tool connection, kept outside retained launch/home payloads.
//! The source must prove channel-created provenance before returning a binding.

use crate::{AdapterError, AgentOpenRequest, RuntimeCredential, RuntimeHandle};

#[derive(Clone, PartialEq, Eq)]
pub struct HostedChannelToolBinding {
    endpoint: String,
    credential: RuntimeCredential,
    runtime: RuntimeHandle,
    agent: crate::AgentSlot,
    request_sha256: String,
    authority_fence: u64,
    retained_conversation: Option<String>,
}

impl std::fmt::Debug for HostedChannelToolBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostedChannelToolBinding(<protected>)")
    }
}

impl HostedChannelToolBinding {
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
        endpoint.set_path("/api/v1/channel-tools");
        Ok(Self {
            endpoint: endpoint.to_string(),
            credential,
            runtime,
            agent: request.options.slot.clone(),
            request_sha256: request.request_sha256.clone(),
            authority_fence,
            retained_conversation,
        })
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

    pub(super) fn acp_server(&self) -> serde_json::Value {
        use agent_client_protocol_schema::v1::{HttpHeader, McpServer, McpServerHttp};
        let server = McpServer::Http(McpServerHttp::new("cma_channel", &self.endpoint).headers(
            vec![HttpHeader::new(
                "Authorization",
                format!("Bearer {}", self.credential.expose()),
            )],
        ));
        serde_json::to_value(server).expect("typed MCP HTTP descriptor serializes")
    }

    pub(super) fn acp_server_plan(&self) -> crate::Result<crate::ingress_budget::AllocationPlan> {
        super::acp::ingress::identity::http_descriptor_plan(
            self.endpoint.len(),
            Some(self.credential.expose().len()),
        )
    }
}

/// Resolve from current retained creation/allocation/authority records. Manual,
/// Trigger and unknown-origin Agents return None even when a bridge uses them.
/// Tool invocation authorization additionally binds the active channel input;
/// this connection capability never chooses a bot, recipient or reply key.
#[async_trait::async_trait]
pub trait HostedChannelToolSource: Send + Sync {
    async fn resolve(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        authority_fence: u64,
    ) -> crate::Result<Option<HostedChannelToolBinding>>;

    /// Persist only after the actual handshake and exact launch receipt pass.
    /// A failed/unknown open cannot be recorded as installed tool configuration.
    async fn confirm_open(
        &self,
        binding: &HostedChannelToolBinding,
        conversation: &str,
    ) -> crate::Result<()>;
}

fn valid_tool_credential(value: &str) -> bool {
    value.strip_prefix("cma_ct_").is_some_and(|suffix| {
        suffix.len() == 64
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("channel tool connection binding is invalid".into())
}
