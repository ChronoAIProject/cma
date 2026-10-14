use super::*;
use serde_json::json;

#[path = "agent_lifecycle_workspace_mongo_tests.rs"]
mod mongo;

#[path = "agent_lifecycle_bootstrap_tests.rs"]
mod bootstrap;

fn request(value: serde_json::Value) -> AgentLifecycleCreateRequest {
    serde_json::from_value(value).unwrap()
}

#[test]
fn contextual_requests_inherit_image_and_only_existing_key_scope() {
    let valid = request(json!({"name":"Second Agent", "workspace_id":"wk_existing"}));
    validate_request(&valid).unwrap();
    validate_scopes(
        &valid,
        &[
            layer2_core::ServiceId::new("github"),
            layer2_core::ServiceId::new("chrono-llm-public"),
            layer2_core::ServiceId::new("ornn-api"),
        ],
    )
    .unwrap();
    let subset = request(
        json!({"name":"Second Agent","workspace_id":"wk_existing", "profile":{"permissions":{"services":["github"]}}}),
    );
    validate_scopes(
        &subset,
        &[
            layer2_core::ServiceId::new("github"),
            layer2_core::ServiceId::new("calendar"),
            layer2_core::ServiceId::new("chrono-llm-public"),
            layer2_core::ServiceId::new("ornn-api"),
        ],
    )
    .unwrap();
    assert!(matches!(
        validate_scopes(&subset, &[]),
        Err(AgentLifecycleProvisioningError::Invalid(_))
    ));
    for invalid in [
        json!({"name":"Second Agent","workspace_id":"wk_existing","image":{"id":"image"}}),
        json!({"name":"Second Agent","workspace_id":"wk_existing","sandbox_id":"sbx_existing"}),
        json!({"name":"Second Agent","workspace_id":""}),
        json!({"name":"Second Agent","workspace_id":"wk /other"}),
    ] {
        assert!(matches!(
            validate_request(&request(invalid)),
            Err(AgentLifecycleProvisioningError::Invalid(_))
        ));
    }
}

#[test]
fn contextual_permission_and_readiness_errors_do_not_retry_forever() {
    use layer2_core::sandbox::AgentServiceError;
    assert!(matches!(
        contextual_error(AgentServiceError::WorkspaceNotFound, "wk_hidden"),
        AgentLifecycleProvisioningError::Forbidden
    ));
    assert!(matches!(
        contextual_error(AgentServiceError::WorkspaceNotReady, "wk_pending"),
        AgentLifecycleProvisioningError::Conflict(_)
    ));
    assert!(matches!(
        contextual_error(
            AgentServiceError::LimitReached { limit: 1, live: 1 },
            "wk_full"
        ),
        AgentLifecycleProvisioningError::Conflict(_)
    ));
}
