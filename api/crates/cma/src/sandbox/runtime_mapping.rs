use std::collections::BTreeMap;

use layer2_core::profiles::{ModelAccess, PermissionLevel, ThinkingEffort};
use layer2_core::sandbox::{Agent, Sandbox, Worktree};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::{
    AdapterCapabilities, AgentSlot, RuntimeOptions, SandboxCapacity, SandboxSpec,
};

pub(super) fn runtime_options(
    agent: &Agent,
    sandbox: &Sandbox,
    worktree: &Worktree,
) -> RuntimeOptions {
    RuntimeOptions {
        model: agent.profile.content.model.clone(),
        reasoning_effort: match agent.profile.content.thinking_effort {
            ThinkingEffort::Minimal => layer3_adapter::ReasoningEffort::Minimal,
            ThinkingEffort::Low => layer3_adapter::ReasoningEffort::Low,
            ThinkingEffort::Medium => layer3_adapter::ReasoningEffort::Medium,
            ThinkingEffort::High => layer3_adapter::ReasoningEffort::High,
            ThinkingEffort::XHigh => layer3_adapter::ReasoningEffort::XHigh,
        },
        permission: match agent.profile.content.permission_level {
            PermissionLevel::Guarded => layer3_adapter::PermissionLevel::Guarded,
            PermissionLevel::Autonomous => layer3_adapter::PermissionLevel::Autonomous,
        },
        own_credential: sandbox.profile.content.model_access == ModelAccess::OwnCredentials,
        working_directory: worktree.path.clone(),
        home_directory: format!("/home/cma/.cma/agents/{}/home", agent.id),
        slot: AgentSlot::new(agent.id.to_string()),
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

pub(super) fn sandbox_spec(sandbox: &Sandbox, deployment_id: &str) -> SandboxSpec {
    SandboxSpec {
        sandbox_id: sandbox.id.to_string(),
        capacity: SandboxCapacity {
            cpu_millis: sandbox.profile.content.cpu_millis,
            memory_mib: sandbox.profile.content.memory_mib,
        },
        environment: sandbox
            .profile
            .content
            .environment_variables
            .iter()
            .map(|variable| (variable.name.clone(), variable.value.clone()))
            .collect::<BTreeMap<_, _>>(),
        reaches: sandbox
            .credential
            .reaches
            .iter()
            .map(ToString::to_string)
            .collect(),
        own_credential: sandbox.profile.content.model_access == ModelAccess::OwnCredentials,
        created_by: Some(deployment_id.into()),
        ..SandboxSpec::default()
    }
}
