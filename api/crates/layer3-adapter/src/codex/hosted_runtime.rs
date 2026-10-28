//! Codex-specific rendering for the shared hosted adapter.
use super::*;

pub(super) struct CodexRuntime {
    pub(super) config: CodexConfig,
}

impl HostedAgentRuntime for CodexRuntime {
    fn home_plan(&self, request: &AgentHomeRequest) -> crate::Result<AgentHomePlan> {
        render_agent_home(request, &self.config)
    }

    fn launch(&self, request: &AgentOpenRequest) -> crate::Result<AgentLaunch> {
        render_launch(request)
    }

    fn open_settings(&self, request: &AgentOpenRequest) -> crate::Result<HostedAgentOpenSettings> {
        let permission_mode = match request.options.permission {
            crate::PermissionLevel::Guarded => "read-only",
            crate::PermissionLevel::Autonomous => "agent-full-access",
        };
        Ok(HostedAgentOpenSettings {
            // ACP 1.4.0 wraps Codex 0.147.0's native ResponseError using its
            // internal-error envelope. Other initialize errors are not evidence
            // of a retained bridge, even when they share that outer code.
            already_initialized_error: Some(serde_json::json!({
                "code": -32603,
                "message": "Internal error",
                "data": { "details": "Already initialized" },
            })),
            open_completion: Some(crate::hosted::HostedOpenCompletionProtocol::V1),
            context_refresh: None,
            permission_mode: Some(permission_mode.into()),
            own_credential: request.options.own_credential,
            channel_tool: None,
            provider_tool: None,
            access_tool: None,
            recovered_prompt: None,
        })
    }
}
