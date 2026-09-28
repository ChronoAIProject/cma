use super::*;
use crate::profiles::{
    content_digest, AgentProvider, ModelAccess, PermissionLevel, ProfileCapacityBounds,
    ThinkingEffort,
};
use serde_json::{json, Value};

fn bounds() -> ProfileCapacityBounds {
    ProfileCapacityBounds {
        min_cpu_millis: 250,
        max_cpu_millis: 8000,
        min_memory_mib: 512,
        max_memory_mib: 16384,
    }
}
fn input() -> Value {
    json!({
        "schema_version":1,
        "metadata":{"name":"Build helper","description":"A retained profile","owner":{"kind":"human","subject":"owner-one"},"provider":"registered"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"supported","model":"test-model","thinking_effort":"very_high"},
        "permission":{"action_mode":"autonomous","service_scope":["service-one"]},
        "context":{"skills":[{"source":"ornn","id":"skill-one","version":"1"}],"system_prompt":"Summarize results.","mcp_servers":[]},
        "environment":{"size":{"cpu_cores":0.5,"memory_gib":1.5},"workspace":{"type":"directory","name":"builds"},
            "setup_commands":[{"phase":"before_workspace","command":"prepare-runtime"},{"phase":"after_workspace","command":"prepare-workspace"}],
            "environment_variables":[{"name":"BUILD_MODE","value":"strict"}]}
    })
}
fn map(value: Value) -> Result<BindingProfilePlan, crate::CoreError> {
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    map_profile(
        value,
        &digest,
        AgentProvider::default(),
        "supported",
        bounds(),
    )
}

#[test]
fn binding_profile_maps_resources_commands_identity_and_agent_settings() {
    let plan = map(input()).unwrap();
    assert!(plan.refused.is_empty(), "{:?}", plan.refused);
    assert_eq!(plan.owner.as_str(), "owner-one");
    assert_eq!(plan.agent.model.as_deref(), Some("test-model"));
    assert_eq!(plan.agent.thinking_effort, ThinkingEffort::XHigh);
    assert_eq!(plan.agent.permission_level, PermissionLevel::Autonomous);
    assert_eq!(plan.agent.system_prompt, "Summarize results.");
    assert_eq!(plan.agent.skills[0].id.as_str(), "skill-one");
    let env = plan.environment.unwrap();
    assert_eq!(
        (env.sandbox.cpu_millis, env.sandbox.memory_mib),
        (500, 1536)
    );
    assert_eq!(env.sandbox.model_access, ModelAccess::ChronoaiManaged);
    assert_eq!(env.sandbox.nyxid_services.len(), 2);
    assert_eq!(env.sandbox.software_install_commands, ["prepare-runtime"]);
    assert_eq!(env.workspace.setup_commands, ["prepare-workspace"]);
    assert_eq!(env.workspace.workspace_name().unwrap(), "builds");
    assert_eq!(env.sandbox.environment_variables[0].name, "BUILD_MODE");
    assert!(env.workspace.agent_profiles.is_empty());
}

#[test]
fn isolated_user_sessions_preserve_parent_environment_and_report_support() {
    let original = map(input()).unwrap();
    let mut value = input();
    value["environment"]["isolated_agents"] = json!(true);
    value["environment"]["session_affinity"] = json!("source_user");
    let isolated = map(value.clone()).unwrap();
    assert_eq!(original.environment, isolated.environment);
    assert!(!original.agent.isolated_agent);
    assert!(isolated.agent.isolated_agent);
    assert_eq!(isolated.session_affinity, SessionAffinity::SourceUser);
    assert!(isolated
        .honoured
        .contains(&"environment.isolated_agents".into()));
    assert!(isolated
        .honoured
        .contains(&"environment.session_affinity".into()));
    value["model"]["access"] = json!("custom");
    assert!(
        map(value).is_err(),
        "isolation cannot expose a shared model credential"
    );
}

#[test]
fn binding_profile_exposes_every_unsupported_requirement() {
    let mut value = input();
    value["model"]["api_type"] = "unsupported".into();
    value["model"]["service"] = "unavailable-service".into();
    value["context"]["mcp_servers"] = json!([{"url":"https://tools.example"}]);
    value["environment"]["setup_commands"][0]["phase"] = "after_agent".into();
    let plan = map(value).unwrap();
    assert_eq!(
        plan.refused
            .iter()
            .map(|r| r.field.as_str())
            .collect::<Vec<_>>(),
        [
            "context.mcp_servers",
            "environment.setup_commands[0]",
            "model.api_type",
            "model.service"
        ]
    );
    assert_eq!(plan.refused[2].reason, "only supported");
    for refused in &plan.refused {
        assert!(!plan.honoured.contains(&refused.field));
    }
    assert!(plan
        .environment
        .unwrap()
        .sandbox
        .software_install_commands
        .is_empty());
    let mut value = input();
    value.as_object_mut().unwrap().remove("environment");
    let plan = map(value).unwrap();
    assert!(plan.environment.is_none());
    assert_eq!(plan.refused[0].field, "environment");
}

#[test]
fn binding_profile_maps_own_credentials_and_repository_workspace() {
    let mut value = input();
    value["model"]["access"] = "custom".into();
    value["permission"]["action_mode"] = "guarded".into();
    value["environment"]["workspace"] = json!({"type":"git_repository","url":"https://github.com/example/project.git","reference":"main"});
    let plan = map(value).unwrap();
    assert_eq!(plan.agent.permission_level, PermissionLevel::Guarded);
    let env = plan.environment.unwrap();
    assert_eq!(env.sandbox.model_access, ModelAccess::OwnCredentials);
    assert_eq!(env.workspace.workspace_name().unwrap(), "project");
}

#[test]
fn binding_profile_refuses_model_selection_overridden_by_service_scope() {
    let mut value = input();
    let services = AgentProvider::default().connection_services();
    value["model"]["service"] = services.legacy_managed.into();
    value["permission"]["service_scope"] = json!([services.managed]);
    let plan = map(value).unwrap();
    assert_eq!(plan.refused.len(), 1);
    assert_eq!(plan.refused[0].field, "model.service");
    assert!(!plan.honoured.contains(&"model.service".to_string()));
    assert_eq!(plan.environment.unwrap().sandbox.nyxid_services.len(), 1);
}

#[test]
fn binding_profile_rejects_digest_mismatch_unknown_fields_and_invalid_bounds() {
    assert!(map_profile(
        input(),
        "wrong",
        AgentProvider::default(),
        "supported",
        bounds()
    )
    .is_err());
    for (path, replacement) in [
        ("/schema_version", json!(2)),
        ("/metadata/owner/subject", json!("")),
        ("/model/access", json!("unrecognized")),
        ("/model/thinking_effort", json!("unrecognized")),
        ("/permission/action_mode", json!("unrecognized")),
        ("/environment/size/cpu_cores", json!(0)),
        ("/environment/size/cpu_cores", json!(0.0001)),
        ("/environment/size/cpu_cores", json!(16)),
        ("/environment/size/memory_gib", json!(64)),
        ("/environment/environment_variables/0/name", json!("HOME")),
        ("/environment/workspace/name", json!("../escape")),
        ("/context/system_prompt", json!("x".repeat(32769))),
    ] {
        let mut value = input();
        *value.pointer_mut(path).unwrap() = replacement;
        assert!(map(value).is_err(), "accepted {path}");
    }
    let mut value = input();
    value["environment"]["unexpected"] = true.into();
    assert!(map(value).is_err());
}

#[test]
fn organization_and_keep_alive_are_explicit_and_legacy_plans_remain_compatible() {
    let legacy = map(input()).unwrap();
    let encoded = serde_json::to_value(&legacy).unwrap();
    assert!(encoded.get("organization_owned").is_none());
    assert!(encoded["environment"].get("always_keep_alive").is_none());
    assert_eq!(
        serde_json::from_value::<BindingProfilePlan>(encoded).unwrap(),
        legacy
    );
    let mut value = input();
    value["metadata"]["owner"]["kind"] = "organization".into();
    value["environment"]["always_keep_alive"] = true.into();
    let plan = map(value).unwrap();
    assert!(plan.organization_owned);
    assert!(plan.environment.as_ref().unwrap().always_keep_alive);
    assert!(plan
        .honoured
        .contains(&"environment.always_keep_alive".into()));
}
