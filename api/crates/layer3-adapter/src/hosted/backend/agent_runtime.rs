use crate::{AgentHomePlan, AgentHomeRequest, AgentLaunch, AgentOpenRequest, Result};

/// Kernel-specific settings used while opening its ACP conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedAgentOpenSettings {
    /// Trusted transport policy; the provider renderer cannot override it.
    pub coordinated_ingress: bool,
    pub retained_json: bool,
    pub bounded_raw_output: bool,
    /// Transport-owned compact permission retention policy.
    pub bounded_permission_state: bool,
    /// Transport policy, overwritten from trusted composition before attachment.
    pub output_json_budget: bool,
    /// Transport-owned policy for immutable retained domain payloads.
    pub shared_runtime_output: bool,
    /// Exact provider-proved initialize error for an already initialized bridge.
    /// This permits reattachment only; it never supplies conversation/tool authority.
    pub already_initialized_error: Option<serde_json::Value>,
    /// Provider-proved read-only evidence of an exact completed conversation open.
    pub open_completion: Option<super::super::HostedOpenCompletionProtocol>,
    /// Trusted control-plane binding; never supplied by a provider renderer.
    pub context_refresh: Option<super::super::managed_context::HostedContextRefresh>,
    pub permission_mode: Option<String>,
    pub own_credential: bool,
    pub channel_tool: Option<super::super::HostedChannelToolBinding>,
    pub provider_tool: Option<super::super::HostedProviderToolBinding>,
    pub access_tool: Option<super::super::HostedAccessToolBinding>,
    /// Control-plane correlation for an already admitted turn; not launch data.
    pub recovered_prompt: Option<crate::PromptIdentity>,
    /// Control-plane admitted context replay; input stays restricted to this
    /// operation until its tools and settings have been acknowledged.
    pub recovered_context_sequence: Option<u64>,
}
/// Provider-specific rendering needed by the otherwise shared hosted adapter.
pub trait HostedAgentRuntime: Send + Sync {
    fn home_plan(&self, request: &AgentHomeRequest) -> Result<AgentHomePlan>;
    fn launch(&self, request: &AgentOpenRequest) -> Result<AgentLaunch>;
    fn open_settings(&self, request: &AgentOpenRequest) -> Result<HostedAgentOpenSettings> {
        Ok(HostedAgentOpenSettings {
            coordinated_ingress: false,
            retained_json: false,
            bounded_raw_output: false,
            bounded_permission_state: false,
            output_json_budget: false,
            shared_runtime_output: false,
            already_initialized_error: None,
            open_completion: None,
            context_refresh: None,
            permission_mode: None,
            own_credential: request.options.own_credential,
            channel_tool: None,
            provider_tool: None,
            access_tool: None,
            recovered_prompt: None,
            recovered_context_sequence: None,
        })
    }
}
