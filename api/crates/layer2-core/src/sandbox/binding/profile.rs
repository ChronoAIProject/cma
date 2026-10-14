use super::profile_input::{Environment, ExternalProfile};
use crate::profiles::{
    content_digest, AgentProfileContent, AgentProvider, ModelAccess, PermissionLevel,
    ProfileCapacityBounds, SandboxProfileContent, ThinkingEffort, WorkspaceLocation,
    WorkspaceProfileContent,
};
use crate::{CoreError, ServiceId, SubjectId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// CMA owns agent affinity. Gateway conversation identifiers remain transport context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAffinity {
    #[default]
    Conversation,
    SourceUser,
}

impl SessionAffinity {
    pub fn is_default(&self) -> bool {
        *self == Self::Conversation
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingFieldRefusal {
    pub field: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingEnvironmentPlan {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub always_keep_alive: bool,
    pub sandbox: SandboxProfileContent,
    pub workspace: WorkspaceProfileContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingProfilePlan {
    #[serde(default, skip_serializing_if = "SessionAffinity::is_default")]
    pub session_affinity: SessionAffinity,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub organization_owned: bool,
    pub owner: SubjectId,
    pub name: String,
    pub profile_digest: String,
    pub environment: Option<BindingEnvironmentPlan>,
    pub agent: AgentProfileContent,
    pub honoured: Vec<String>,
    pub refused: Vec<BindingFieldRefusal>,
}

/// Validate a caller's snapshot and project it into the existing profile model.
/// Capability policy is supplied by composition, never inferred from client metadata.
pub fn map_profile(
    value: Value,
    expected_digest: &str,
    provider: AgentProvider,
    supported_model_api: &str,
    bounds: ProfileCapacityBounds,
) -> Result<BindingProfilePlan, CoreError> {
    let digest = format!("sha256:{}", content_digest(&value).map_err(invalid)?);
    if digest != expected_digest {
        return Err(invalid(
            "profile_digest does not match the supplied profile",
        ));
    }
    if serde_json::to_vec(&value).map_err(invalid)?.len() > 262_144 {
        return Err(invalid("profile exceeds 256 KiB"));
    }
    if value.get("environment").is_some_and(|environment| {
        serde_json::to_vec(environment).is_ok_and(|bytes| bytes.len() > 65_536)
    }) {
        return Err(invalid("environment exceeds 64 KiB"));
    }
    let input: ExternalProfile =
        serde_json::from_value(value).map_err(|_| invalid("invalid external profile schema"))?;
    if input.schema_version != 1 {
        return Err(invalid("unsupported profile schema_version"));
    }
    bounded(&input.metadata.name, "metadata.name", 1, 80)?;
    bounded(&input.metadata.description, "metadata.description", 0, 2000)?;
    bounded(
        &input.metadata.owner.subject,
        "metadata.owner.subject",
        1,
        512,
    )?;
    bounded(&input.metadata.provider, "metadata.provider", 1, 32)?;
    if !matches!(
        input.metadata.owner.kind.as_str(),
        "human" | "service_account" | "agent_key" | "organization"
    ) {
        return Err(invalid("invalid metadata.owner.kind"));
    }
    bounded(&input.model.model, "model.model", 1, 512)?;
    bounded(&input.model.service, "model.service", 1, 512)?;
    let model_access = match input.model.access.as_str() {
        "managed" => ModelAccess::ChronoaiManaged,
        "custom" => ModelAccess::OwnCredentials,
        _ => return Err(invalid("invalid model.access")),
    };
    let thinking_effort = match input.model.thinking_effort.as_str() {
        "minimal" => ThinkingEffort::Minimal,
        "low" => ThinkingEffort::Low,
        "medium" => ThinkingEffort::Medium,
        "high" => ThinkingEffort::High,
        "very_high" => ThinkingEffort::XHigh,
        _ => return Err(invalid("invalid model.thinking_effort")),
    };
    let permission_level = match input.permission.action_mode.as_str() {
        "guarded" => PermissionLevel::Guarded,
        "autonomous" => PermissionLevel::Autonomous,
        _ => return Err(invalid("invalid permission.action_mode")),
    };
    if input.permission.service_scope.len() > 64 {
        return Err(invalid("permission.service_scope exceeds 64 services"));
    }
    let mut services = BTreeSet::new();
    for service in input.permission.service_scope {
        bounded(&service, "permission.service_scope", 1, 512)?;
        services.insert(ServiceId::new(service));
    }
    let session_affinity = input
        .environment
        .as_ref()
        .map_or(SessionAffinity::default(), |e| e.session_affinity);
    let agent = AgentProfileContent {
        isolated_agent: input
            .environment
            .as_ref()
            .is_some_and(|e| e.isolated_agents),
        model: Some(input.model.model),
        thinking_effort,
        permission_level,
        skills: input.context.skills,
        system_prompt: input.context.system_prompt,
    };
    if agent.isolated_agent && model_access != ModelAccess::ChronoaiManaged {
        return Err(invalid("isolated agents require managed model access"));
    }
    agent.validate().map_err(invalid)?;
    for skill in &agent.skills {
        bounded(skill.id.as_str(), "context.skills.id", 1, 512)?;
        if let Some(version) = &skill.version {
            bounded(version, "context.skills.version", 1, 128)?;
        }
    }
    let mut plan = BindingProfilePlan {
        session_affinity,
        organization_owned: input.metadata.owner.kind == "organization",
        owner: SubjectId::new(input.metadata.owner.subject),
        name: input.metadata.name,
        profile_digest: digest,
        environment: None,
        agent,
        honoured: [
            "metadata",
            "model.access",
            "model.model",
            "model.thinking_effort",
            "permission.action_mode",
            "permission.service_scope",
            "context.skills",
            "context.system_prompt",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        refused: Vec::new(),
    };
    if plan.agent.isolated_agent {
        plan.honoured.push("environment.isolated_agents".into());
    }
    if plan.session_affinity == SessionAffinity::SourceUser {
        plan.honoured.push("environment.session_affinity".into());
    }
    if input.model.api_type == supported_model_api {
        plan.honoured.push("model.api_type".into());
    } else {
        plan.refuse("model.api_type", format!("only {supported_model_api}"));
    }
    if input.context.external_tools.is_empty() {
        plan.honoured.push("context.mcp_servers".into());
    } else {
        plan.refuse("context.mcp_servers", "not supported in this milestone");
    }
    // Managed model selection follows the retained service scope in the runtime adapter.
    let model_services = provider.connection_services();
    if model_access == ModelAccess::ChronoaiManaged
        && [model_services.managed, model_services.legacy_managed]
            .contains(&input.model.service.as_str())
    {
        if input.model.service == model_services.legacy_managed
            && services.contains(&ServiceId::new(model_services.managed))
        {
            plan.refuse(
                "model.service",
                "service_scope selects another managed model service",
            );
        } else {
            services.insert(ServiceId::new(input.model.service));
            plan.honoured.push("model.service".into());
        }
    } else {
        plan.refuse(
            "model.service",
            "model service cannot be selected for this access mode or provider",
        );
    }
    match input.environment {
        Some(environment) => {
            plan.environment = Some(map_environment(
                environment,
                provider,
                model_access,
                services,
                bounds,
                &mut plan,
            )?)
        }
        None => plan.refuse("environment", "environment required for this provider"),
    }
    plan.honoured.sort();
    plan.refused.sort_by(|a, b| a.field.cmp(&b.field));
    Ok(plan)
}

fn map_environment(
    input: Environment,
    provider: AgentProvider,
    model_access: ModelAccess,
    services: BTreeSet<ServiceId>,
    bounds: ProfileCapacityBounds,
    plan: &mut BindingProfilePlan,
) -> Result<BindingEnvironmentPlan, CoreError> {
    let cpu_millis = units(input.size.cpu_cores, 1000.0, "environment.size.cpu_cores")?;
    let memory_mib = units(input.size.memory_gib, 1024.0, "environment.size.memory_gib")?;
    let mut before = Vec::new();
    let mut after = Vec::new();
    if input.setup_commands.len() > 48 {
        return Err(invalid("too many environment.setup_commands"));
    }
    for (index, setup) in input.setup_commands.into_iter().enumerate() {
        let field = format!("environment.setup_commands[{index}]");
        match setup.phase.as_str() {
            "before_workspace" => before.push(setup.command),
            "after_workspace" => after.push(setup.command),
            _ => {
                plan.refuse(&field, "unsupported setup phase");
                continue;
            }
        }
        plan.honoured.push(field);
    }
    let name = match &input.workspace {
        WorkspaceLocation::Directory { name } => name.clone(),
        WorkspaceLocation::GitRepository { .. } => String::new(),
    };
    let workspace = WorkspaceProfileContent {
        name,
        location: input.workspace,
        setup_commands: after,
        agent_profiles: Vec::new(),
    };
    workspace.validate().map_err(invalid)?;
    let sandbox = SandboxProfileContent {
        runtime_image: None,
        cpu_millis,
        memory_mib,
        nyxid_services: services.into_iter().collect(),
        agent_provider: provider,
        model_access,
        max_workspaces: 1,
        max_agents_per_workspace: 16,
        workspace_profiles: Vec::new(),
        software_install_commands: before,
        environment_variables: input.environment_variables,
    };
    sandbox.validate(bounds).map_err(invalid)?;
    plan.honoured.extend(
        [
            "environment.size",
            "environment.workspace",
            "environment.environment_variables",
        ]
        .map(str::to_owned),
    );
    if input.always_keep_alive {
        plan.honoured.push("environment.always_keep_alive".into());
    }
    Ok(BindingEnvironmentPlan {
        sandbox,
        workspace,
        always_keep_alive: input.always_keep_alive,
    })
}

impl BindingProfilePlan {
    fn refuse(&mut self, field: &str, reason: impl Into<String>) {
        self.refused.push(BindingFieldRefusal {
            field: field.into(),
            reason: reason.into(),
        });
    }
}
fn units(value: f64, factor: f64, field: &str) -> Result<u32, CoreError> {
    let units = value * factor;
    if !units.is_finite() || units < 1.0 || units > u32::MAX as f64 || units.fract() != 0.0 {
        return Err(invalid(format!(
            "{field} must map to a positive whole resource unit"
        )));
    }
    Ok(units as u32)
}
fn bounded(value: &str, field: &str, min: usize, max: usize) -> Result<(), CoreError> {
    if value.len() < min || value.len() > max || value.contains('\0') {
        return Err(invalid(format!("{field} exceeds its bounds")));
    }
    Ok(())
}
fn invalid(reason: impl std::fmt::Display) -> CoreError {
    CoreError::InvalidRequest(reason.to_string())
}
