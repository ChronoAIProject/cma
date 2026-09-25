//! Provider-neutral Event Worker Profile projection from immutable CMA revisions.
use crate::{
    bots::{BotError, BotProfiles},
    profiles::{ModelAccess, PermissionLevel, ThinkingEffort},
    ServiceId, SubjectId,
};
use serde_json::{json, Value};

pub struct WorkerProjection<'a> {
    pub name: &'a str,
    pub owner: &'a SubjectId,
    pub provider: &'a str,
    pub profiles: &'a BotProfiles,
}

pub fn services(bot: &WorkerProjection<'_>) -> Result<Vec<ServiceId>, BotError> {
    let sandbox = &bot.profiles.sandbox.content;
    if sandbox.model_access != ModelAccess::ChronoaiManaged {
        return Err(BotError::Invalid(
            "CMAEG bots require a managed model connection".into(),
        ));
    }
    let mut services = sandbox.nyxid_services.clone();
    let model = ServiceId::new(model_service(bot));
    if !services.contains(&model) {
        services.push(model);
    }
    services.sort();
    Ok(services)
}
fn model_service(bot: &WorkerProjection<'_>) -> &'static str {
    let sandbox = &bot.profiles.sandbox.content;
    let model = sandbox.agent_provider.connection_services();
    if sandbox
        .nyxid_services
        .contains(&ServiceId::new(model.managed))
    {
        model.managed
    } else if sandbox
        .nyxid_services
        .contains(&ServiceId::new(model.legacy_managed))
    {
        model.legacy_managed
    } else {
        model.managed
    }
}
pub fn snapshot(bot: &WorkerProjection<'_>) -> Result<Value, BotError> {
    snapshot_for_owner(
        bot,
        &crate::resource_access::ResourceOwner::Person {
            subject: bot.owner.clone(),
        },
    )
}
pub fn snapshot_for_owner(
    bot: &WorkerProjection<'_>,
    owner: &crate::resource_access::ResourceOwner,
) -> Result<Value, BotError> {
    if match owner {
        crate::resource_access::ResourceOwner::Person { subject }
        | crate::resource_access::ResourceOwner::Organization { subject } => subject,
    } != bot.owner
    {
        return Err(BotError::Forbidden);
    }
    let owner_kind = match owner {
        crate::resource_access::ResourceOwner::Person { .. } => "human",
        crate::resource_access::ResourceOwner::Organization { .. } => "organization",
    };
    let agent = &bot.profiles.agent.content;
    let sandbox = &bot.profiles.sandbox.content;
    let workspace = &bot.profiles.workspace.content;
    let setup: Vec<_> = sandbox
        .software_install_commands
        .iter()
        .map(|command| json!({"phase":"before_workspace","command":command}))
        .chain(
            workspace
                .setup_commands
                .iter()
                .map(|command| json!({"phase":"after_workspace","command":command})),
        )
        .collect();
    Ok(json!({
        "schema_version":1,
        "metadata":{"name":bot.name,"description":"", "owner":{"subject":bot.owner,"kind":owner_kind},"provider":bot.provider},
        "model":{"access":"managed","service":model_service(bot),"api_type":"responses","model":bot.profiles.resolved_model,
            "thinking_effort":match agent.thinking_effort { ThinkingEffort::Minimal => "minimal", ThinkingEffort::Low => "low", ThinkingEffort::Medium => "medium", ThinkingEffort::High => "high", ThinkingEffort::XHigh => "very_high" }},
        "permission":{"action_mode":match agent.permission_level { PermissionLevel::Guarded => "guarded", PermissionLevel::Autonomous => "autonomous" },"service_scope":services(bot)?},
        "context":{"system_prompt":agent.system_prompt,"skills":agent.skills,"mcp_servers":[]},
        "environment":{"size":{"cpu_cores":f64::from(sandbox.cpu_millis)/1000.0,"memory_gib":f64::from(sandbox.memory_mib)/1024.0},
            "workspace":workspace.location,"setup_commands":setup,"environment_variables":sandbox.environment_variables}
    }))
}
