//! Independent HTTP projection of pinned CMA profiles into CMAEG protocol 1.
use layer2_core::{
    bots::{Bot, BotError},
    profiles::{ModelAccess, PermissionLevel, ThinkingEffort},
    ServiceId,
};
use serde_json::{json, Value};

pub(super) fn services(bot: &Bot) -> Result<Vec<ServiceId>, BotError> {
    let sandbox = &bot.profiles.sandbox.content;
    if sandbox.model_access != ModelAccess::ChronoaiManaged {
        return Err(BotError::Invalid(
            "CMAEG bots require a managed model connection".into(),
        ));
    }
    let mut services = sandbox.nyxid_services.clone();
    for service in [model_service(bot), "cmaeg"] {
        let service = ServiceId::new(service);
        if !services.contains(&service) {
            services.push(service);
        }
    }
    services.sort();
    Ok(services)
}
fn model_service(bot: &Bot) -> &'static str {
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
pub(super) fn snapshot(bot: &Bot) -> Result<Value, BotError> {
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
        "metadata":{"name":bot.request.name,"description":"", "owner":{"subject":bot.owner,"kind":"human"},"provider":bot.provider},
        "model":{"access":"managed","service":model_service(bot),"api_type":"responses","model":bot.profiles.resolved_model,
            "thinking_effort":match agent.thinking_effort { ThinkingEffort::Minimal => "minimal", ThinkingEffort::Low => "low", ThinkingEffort::Medium => "medium", ThinkingEffort::High => "high", ThinkingEffort::XHigh => "very_high" }},
        "permission":{"action_mode":match agent.permission_level { PermissionLevel::Guarded => "guarded", PermissionLevel::Autonomous => "autonomous" },"service_scope":services(bot)?},
        "context":{"system_prompt":agent.system_prompt,"skills":agent.skills,"mcp_servers":[]},
        "environment":{"size":{"cpu_cores":f64::from(sandbox.cpu_millis)/1000.0,"memory_gib":f64::from(sandbox.memory_mib)/1024.0},
            "workspace":workspace.location,"setup_commands":setup,"environment_variables":sandbox.environment_variables}
    }))
}
