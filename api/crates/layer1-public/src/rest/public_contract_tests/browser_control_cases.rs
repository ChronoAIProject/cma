use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer3_adapter::*;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt as _;

pub(super) async fn scenario(name: &str) -> Option<super::Scenario> {
    let kind = name.strip_prefix("browser_control_")?;
    let access = match kind {
        "owner" | "viewer" | "writer" | "revoked" | "foreign" | "anonymous" | "paused"
        | "not_ready" => kind,
        _ => "owner",
    };
    super::scenario_with_runtime(
        &format!("filesystem_{access}"),
        Arc::new(super::runtime::Files::new(name)),
    )
    .await
}

pub(super) async fn runtime_reply(
    runtime: &super::runtime::Files,
    request: &BrowserControlRequest,
) -> Result<BrowserControlReceipt> {
    if !runtime.kind.starts_with("browser_control") || runtime.kind.ends_with("unsupported") {
        return Err(AdapterError::Unsupported("control unsupported".into()));
    }
    if runtime.kind.ends_with("conflict") {
        return Err(AdapterError::Rejected("browser_control_conflict".into()));
    }
    if runtime.kind.ends_with("unavailable") {
        return Err(AdapterError::Unavailable("control unavailable".into()));
    }
    assert_eq!(request.actor.len(), 64);
    assert_ne!(request.actor, "owner-a");
    assert!(request.valid());
    let receipt = request.operation_id.is_some();
    let user = receipt && request.command != BrowserControlCommand::Return;
    let revision = request
        .expected_revision
        .map_or(u64::from(receipt), |revision| revision + 1);
    let audit = if runtime.kind.ends_with("client_key") {
        json!([{"revision":revision,"kind":"acquired",
        "actor":request.actor,"client_id":request.client_id,"operation_id":request.operation_id,"at":"2026-11-03T15:00:00Z"}])
    } else {
        json!([])
    };
    let control = serde_json::from_value(json!({"state":if user {"user"} else {"agent"}, "revision":revision,
        "client_id":if user {Some(request.client_id.clone())} else {None},
        "lease_token":if user {Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")} else {None},
        "expires_at":if user {Some("2026-11-03T15:00:15Z")} else {None}, "lease_duration_ms":15000,
        "operation_id":request.operation_id, "operation_revision":if receipt {Some(revision)} else {None}, "audit":audit, "audit_truncated_before":null})).unwrap();
    Ok(BrowserControlReceipt {
        agent_id: request.agent_id.clone(),
        boot_id: request.boot_id.clone(),
        context_id: if runtime.kind.ends_with("mismatch") {
            "79a0e773-7691-401e-89e8-7487345c221a".into()
        } else {
            request
                .context_id
                .clone()
                .unwrap_or("e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24".into())
        },
        control,
    })
}

#[tokio::test]
async fn browser_control_recovery_authority_is_bound_to_an_unpublished_client_key() {
    let runtime = Arc::new(super::runtime::Files::new("browser_control_client_key"));
    let fixture = super::scenario_with_runtime("filesystem_owner", runtime)
        .await
        .unwrap();
    let input = json!({"runtime_incarnation":"72d7e4de8a534f369c98f9043597e7983246b03ce36c64d0b43987d50e3f8821",
        "context_id":"e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24", "client_id":"same-public-client", "command":"acquire",
        "operation_id":"78af4405-b644-44ab-b58d-53ce4d62acc0", "expected_revision":0});
    let mut actors = Vec::new();
    for key in [
        "b2f66e8e-5ddd-4e96-b2e9-84eb55195664",
        "a7d65755-cd0b-4bc0-b253-e583629175f7",
    ] {
        let mut request = crate::rest::agents_tests::request(
            Method::POST,
            "/api/v1/agents/agt_shared_read/browser/control",
            "owner-a",
            None,
            Some(input.clone()),
        );
        request
            .headers_mut()
            .insert("x-cma-browser-client", key.parse().unwrap());
        let response = fixture.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(key));
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        actors.push(value["audit"][0]["actor"].clone());
    }
    assert_ne!(actors[0], actors[1]);
}

#[tokio::test]
async fn browser_control_public_mutation_requires_owner_and_fences_incarnation() {
    let runtime = Arc::new(super::runtime::Files::new("browser_control"));
    let input = json!({"runtime_incarnation":"72d7e4de8a534f369c98f9043597e7983246b03ce36c64d0b43987d50e3f8821",
        "context_id":"e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24", "client_id":"control-client", "command":"acquire",
        "operation_id":"78af4405-b644-44ab-b58d-53ce4d62acc0", "expected_revision":0});
    for (kind, subject, expected) in [
        ("owner", "owner-a", StatusCode::OK),
        ("viewer", "link-recipient", StatusCode::FORBIDDEN),
        ("foreign", "foreign", StatusCode::NOT_FOUND),
    ] {
        let fixture = super::scenario_with_runtime(&format!("filesystem_{kind}"), runtime.clone())
            .await
            .unwrap();
        let mut request = crate::rest::agents_tests::request(
            Method::POST,
            "/api/v1/agents/agt_shared_read/browser/control",
            subject,
            None,
            Some(input.clone()),
        );
        request.headers_mut().insert(
            "x-cma-browser-client",
            "b2f66e8e-5ddd-4e96-b2e9-84eb55195664".parse().unwrap(),
        );
        let response = fixture.router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        assert_eq!(status, expected, "{}", String::from_utf8_lossy(&bytes));
        if kind == "owner" {
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["state"], "user");
            assert_eq!(value["operation_id"], input["operation_id"]);
            assert_eq!(value["context_id"], input["context_id"]);
        }
    }
}
