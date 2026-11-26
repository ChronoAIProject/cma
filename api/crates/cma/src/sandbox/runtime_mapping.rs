use std::collections::BTreeMap;

use layer2_core::profiles::{ModelAccess, PermissionLevel, SandboxProfileSnapshot, ThinkingEffort};
use layer2_core::sandbox::{Agent, Sandbox, Worktree};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::{
    AdapterCapabilities, AgentSlot, RuntimeOptions, SandboxCapacity, SandboxSpec,
};

pub(crate) fn runtime_options(
    agent: &Agent,
    sandbox: &Sandbox,
    worktree: &Worktree,
) -> RuntimeOptions {
    profile_runtime_options(
        &agent.profile.content,
        &sandbox.profile.content,
        &agent.id,
        &worktree.path,
    )
}

pub(super) fn profile_runtime_options(
    agent: &layer2_core::profiles::AgentProfileContent,
    sandbox: &layer2_core::profiles::SandboxProfileContent,
    agent_id: &layer2_core::AgentId,
    worktree_path: &str,
) -> RuntimeOptions {
    RuntimeOptions {
        isolated_agent: agent.isolated_agent,
        model: agent.model.clone(),
        reasoning_effort: match agent.thinking_effort {
            ThinkingEffort::Minimal => layer3_adapter::ReasoningEffort::Minimal,
            ThinkingEffort::Low => layer3_adapter::ReasoningEffort::Low,
            ThinkingEffort::Medium => layer3_adapter::ReasoningEffort::Medium,
            ThinkingEffort::High => layer3_adapter::ReasoningEffort::High,
            ThinkingEffort::XHigh => layer3_adapter::ReasoningEffort::XHigh,
        },
        permission: match agent.permission_level {
            PermissionLevel::Guarded => layer3_adapter::PermissionLevel::Guarded,
            PermissionLevel::Autonomous => layer3_adapter::PermissionLevel::Autonomous,
        },
        own_credential: sandbox.model_access == ModelAccess::OwnCredentials,
        working_directory: worktree_path.into(),
        home_directory: format!("/home/cma/.cma/agents/{agent_id}/home"),
        slot: AgentSlot::new(agent_id.to_string()),
    }
}

pub(crate) fn runtime_capabilities(capabilities: AdapterCapabilities) -> RuntimeCapabilities {
    let values = [
        (capabilities.conversation, RuntimeCapability::Conversation),
        (capabilities.snapshot, RuntimeCapability::Snapshot),
        (capabilities.steer, RuntimeCapability::Steer),
        (capabilities.interrupt, RuntimeCapability::Interrupt),
        (capabilities.usage, RuntimeCapability::Usage),
        (capabilities.resources, RuntimeCapability::Resources),
    ];
    RuntimeCapabilities::new(
        values
            .into_iter()
            .filter_map(|(enabled, capability)| enabled.then_some(capability)),
    )
}

pub(super) fn sandbox_spec(
    sandbox: &Sandbox,
    deployment_id: &str,
) -> layer3_adapter::Result<SandboxSpec> {
    let custom_image = sandbox
        .custom_runtime_image_for_binding()
        .map_err(|error| layer3_adapter::AdapterError::Rejected(error.to_string()))?;
    let mut spec = environment_spec(
        &sandbox.id,
        &sandbox.profile,
        &sandbox.runtime.as_ref().map_or_else(
            || sandbox.credential.effective_reaches(),
            |runtime| sandbox.credential.runtime_reaches(runtime),
        ),
        deployment_id,
    );
    if let layer2_core::sandbox::SandboxCredentialSource::Binding { tool_endpoint, .. } =
        &sandbox.credential.source
    {
        spec.tool_proxy_url = tool_endpoint.clone();
    }
    // The saved profile remains immutable. A queued migration retains the old
    // image; only its validated successor binding can select the admitted target.
    spec.runtime_image = custom_image.map(str::to_owned);
    spec.home_volume = sandbox
        .lifecycle
        .home
        .as_ref()
        .and_then(|home| home.attachment())
        .map(Box::new);
    if let Some(evidence) = &sandbox.lifecycle.retained_startup_policy {
        if spec
            .home_volume
            .as_ref()
            .and_then(|home| home.identity().ok())
            .as_deref()
            == Some(evidence.observation.runtime.home_identity.as_str())
        {
            spec.retained_journal_recovery =
                Some(evidence.observation.legacy_journal_recovery_enabled);
        }
    }
    if spec.runtime_image.is_none() {
        spec.runtime_image = sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.native_pool.as_ref())
            .map(|native| {
                // Original pool authority stays immutable; the current binding
                // records the actual image after an admitted software update.
                sandbox
                    .retained_runtime_binding()
                    .map(|binding| binding.runtime_version.clone())
                    .unwrap_or_else(|| native.request.pool.spec.image.uri.clone())
            });
    }
    Ok(spec)
}

pub(super) fn environment_spec(
    id: &layer2_core::SandboxId,
    profile: &SandboxProfileSnapshot,
    reaches: &[layer2_core::ServiceId],
    deployment_id: &str,
) -> SandboxSpec {
    SandboxSpec {
        sandbox_id: id.to_string(),
        capacity: SandboxCapacity {
            cpu_millis: profile.content.cpu_millis,
            memory_mib: profile.content.memory_mib,
        },
        runtime_image: profile.content.runtime_image.clone(),
        environment: profile
            .content
            .environment_variables
            .iter()
            .map(|variable| (variable.name.clone(), variable.value.clone()))
            .collect::<BTreeMap<_, _>>(),
        reaches: reaches.iter().map(ToString::to_string).collect(),
        own_credential: profile.content.model_access == ModelAccess::OwnCredentials,
        created_by: Some(deployment_id.into()),
        ..SandboxSpec::default()
    }
}
