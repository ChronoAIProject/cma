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

pub(super) const VERIFIED_SENDER_CONTEXT: &str = "environment.verified_sender_context";

pub(super) fn sender_context_request(value: &Value) -> Result<Option<bool>, CoreError> {
    value
        .get("environment")
        .and_then(|v| v.get("verified_sender_context"))
        .map(|v| {
            v.as_bool()
                .ok_or_else(|| invalid("environment.verified_sender_context must be a boolean"))
        })
        .transpose()
}

fn remove_sender_context(value: &mut Value) {
    if let Some(environment) = value.get_mut("environment").and_then(Value::as_object_mut) {
        environment.remove("verified_sender_context");
        if environment.is_empty() {
            value
                .as_object_mut()
                .expect("profile object")
                .remove("environment");
        }
    }
}

/// Compare only independent policy changes, preserving historical absent/null snapshots.
pub(super) fn same_configuration(current: &BindingProfilePlan, requested: &Value) -> bool {
    let mut value = requested.clone();
    remove_sender_context(&mut value);
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    object.remove("queue_lifetime_seconds");
    let matches = |v: &Value| {
        content_digest(v).is_ok_and(|d| format!("sha256:{d}") == current.configuration_digest())
    };
    if matches(&value) {
        return true;
    }
    if value.get("environment").is_none_or(Value::is_null) {
        value["environment"] = Value::Null;
        if matches(&value) {
            return true;
        }
        value
            .as_object_mut()
            .expect("profile object")
            .remove("environment");
        return matches(&value);
    }
    false
}

impl BindingProfilePlan {
    /// The existing negotiation list is also the retained opt-in policy. No new
    /// persisted field is needed, so previous readers can recover admitted work.
    pub fn verified_sender_context_enabled(&self) -> bool {
        self.honoured
            .iter()
            .any(|field| field == VERIFIED_SENDER_CONTEXT)
    }
}

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
    /// Caller configuration digest excluding independent input policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_lifetime_seconds: Option<u32>,
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
    mut value: Value,
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
    let sender_context = sender_context_request(&value)?;
    if sender_context.is_some() {
        remove_sender_context(&mut value);
    }
    let configuration_digest =
        if value.get("queue_lifetime_seconds").is_some() || sender_context.is_some() {
            let mut configuration = value.clone();
            configuration
                .as_object_mut()
                .expect("profile object")
                .remove("queue_lifetime_seconds");
            Some(format!(
                "sha256:{}",
                content_digest(&configuration).map_err(invalid)?
            ))
        } else {
            None
        };
    let input: ExternalProfile =
        serde_json::from_value(value).map_err(|_| invalid("invalid external profile schema"))?;
    if !valid_queue_lifetime(input.queue_lifetime_seconds) {
        return Err(invalid(
            "queue_lifetime_seconds must be between 1 and 86400",
        ));
    }
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
        configuration_digest,
        queue_lifetime_seconds: input.queue_lifetime_seconds,
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

    if sender_context.is_some() {
        plan.refuse(
            VERIFIED_SENDER_CONTEXT,
            "supported only for an existing Agent target",
        );
    }

    if input.queue_lifetime_seconds.is_some() {
        plan.honoured.push("queue_lifetime_seconds".into());
    }
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
        WorkspaceLocation::RepositorySnapshot { .. }
        | WorkspaceLocation::PublicRepositorySnapshot { .. } => "workspace".into(),
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
    pub(super) fn configuration_digest(&self) -> &str {
        self.configuration_digest
            .as_deref()
            .unwrap_or(&self.profile_digest)
    }
    /// Waiting policy for newly admitted messages; retained commands keep their deadline.
    pub fn effective_queue_lifetime_seconds(&self) -> u32 {
        self.queue_lifetime_seconds.unwrap_or(300)
    }

    fn refuse(&mut self, field: &str, reason: impl Into<String>) {
        self.refused.push(BindingFieldRefusal {
            field: field.into(),
            reason: reason.into(),
        });
    }
}
pub(super) fn valid_queue_lifetime(seconds: Option<u32>) -> bool {
    seconds.is_none_or(|seconds| (1..=86400).contains(&seconds))
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
