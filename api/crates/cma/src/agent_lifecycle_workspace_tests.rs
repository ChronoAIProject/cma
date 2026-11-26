use super::*;
use serde_json::json;

#[path = "agent_lifecycle_workspace_fixture.rs"]
pub(in crate::agent_lifecycle) mod fixture;

#[path = "agent_lifecycle_workspace_mongo_tests.rs"]
mod mongo;
#[path = "agent_lifecycle_quota_tests.rs"]
mod quota;

#[path = "agent_lifecycle_bootstrap_tests.rs"]
mod bootstrap;
#[path = "agent_lifecycle_title_tests.rs"]
mod title_tests;

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

#[tokio::test]
async fn lifecycle_callback_distinguishes_unknown_retained_deleted_and_missing_agents() {
    use layer2_core::sandbox::{
        store::{AgentStore, InMemorySandboxStore},
        Agent, AgentStatus,
    };
    let store = InMemorySandboxStore::default();
    let origin = layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap();
    let mut agent: Agent = serde_json::from_value(json!({
        "id":"agt_page", "worktree_id":"wkt_page", "workspace_id":"wks_page", "sandbox_id":"sbx_page",
        "owner":"owner", "creation_key":"page", "creation_fingerprint":"a".repeat(64),
        "profile":{"id":"agp_page", "revision":1, "digest":"b".repeat(64), "content":{
            "thinking_effort":"high", "permission_level":"autonomous"
        }},
        "status":"starting", "conversation":{"last_seq":0,"queued_messages":[]},
        "work":layer2_core::sandbox::AgentWorkState::new(layer2_core::sandbox::AgentWorkStep::None, layer2_core::now_rfc3339()),
        "created_at":"2026-11-04T00:00:00Z", "updated_at":"2026-11-04T00:00:00Z",
        "origin":{"type":"manual"}, "revision":1
    })).unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    for (id, expected) in [
        (None, None),
        (
            Some("agt_page"),
            Some(json!("https://cma.example/agents/agt_page")),
        ),
        (Some("agt_missing"), Some(serde_json::Value::Null)),
        (Some("agt_page"), Some(serde_json::Value::Null)),
    ] {
        if expected == Some(serde_json::Value::Null) && id == Some("agt_page") {
            agent.status = AgentStatus::Deleted;
            agent.revision += 1;
            store.save_agent(1, agent.clone()).await.unwrap();
        }
        let view = AgentLifecycleOperationView {
            accepted_at: None,
            failure: None,
            id: "operation".into(),
            status: "running".into(),
            phase: "agent_startup".into(),
            can_cancel: None,
            agent_id: id.map(Into::into),
            error: None,
            events: vec![],
            connection: None,
            callback_url: progress::callback_url(&store, &origin, "operation", id).await,
        };
        assert_eq!(
            serde_json::to_value(view)
                .unwrap()
                .get("callback_url")
                .cloned(),
            expected
        );
    }
}
