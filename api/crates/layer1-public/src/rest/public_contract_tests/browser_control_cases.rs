use axum::{
    body::to_bytes,
    http::{Method, StatusCode},
};
use layer3_adapter::*;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt as _;

#[tokio::test]
async fn task_experience_default_off_refuses_new_browser_control() {
    let fixture = super::scenario_with_runtime(
        "filesystem_policy_off",
        Arc::new(super::runtime::Files::new("browser_control")),
    )
    .await
    .unwrap();
    let mut request = crate::rest::agents_tests::request(
        Method::POST,
        "/api/v1/agents/agt_shared_read/browser/control",
        "owner-a",
        None,
        Some(
            json!({"runtime_incarnation":"72d7e4de8a534f369c98f9043597e7983246b03ce36c64d0b43987d50e3f8821",
            "context_id":"e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24", "client_id":"policy-client", "command":"acquire",
            "operation_id":"78af4405-b644-44ab-b58d-53ce4d62acc0", "expected_revision":0}),
        ),
    );
    request.headers_mut().insert(
        "x-cma-browser-client",
        "b2f66e8e-5ddd-4e96-b2e9-84eb55195664".parse().unwrap(),
    );
    let response = fixture.router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

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
    if runtime.kind == "browser_control_policy" {
        let receipts = runtime.control_receipts.lock().unwrap();
        if let Some(operation) = &request.operation_id {
            if let Some((original, receipt)) = receipts.get(operation) {
                if original.actor != request.actor
                    || original.client_id != request.client_id
                    || (request.command != BrowserControlCommand::Status && original != request)
                {
                    return Err(AdapterError::Rejected(
                        "conflicting original operation".into(),
                    ));
                }
                return Ok(receipt.clone());
            }
            if request.command == BrowserControlCommand::Status {
                return Err(AdapterError::Rejected("unaccepted operation".into()));
            }
        }
    }
    if !runtime.kind.starts_with("browser_control") || runtime.kind.ends_with("unsupported") {
        return Err(AdapterError::Unsupported("control unsupported".into()));
    }
    if request.command == BrowserControlCommand::Status
        && request.operation_id.is_some()
        && runtime.kind == "browser_control"
    {
        return Err(AdapterError::Rejected("unaccepted operation".into()));
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
    let result = BrowserControlReceipt {
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
    };
    if runtime.kind == "browser_control_policy" && request.command != BrowserControlCommand::Status
    {
        runtime.control_receipts.lock().unwrap().insert(
            request.operation_id.clone().unwrap(),
            (request.clone(), result.clone()),
        );
    }
    Ok(result)
}

#[tokio::test]
async fn task_experience_control_closure_keeps_exact_receipt_and_return_but_blocks_new_input_and_renewal(
) {
    let runtime = Arc::new(super::runtime::Files::new("browser_control_policy"));
    let enabled = super::scenario_with_runtime("filesystem_owner", runtime.clone())
        .await
        .unwrap();
    let closed = super::scenario_with_runtime("filesystem_policy_off", runtime)
        .await
        .unwrap();
    let original = json!({"runtime_incarnation":"72d7e4de8a534f369c98f9043597e7983246b03ce36c64d0b43987d50e3f8821",
        "context_id":"e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24", "client_id":"policy-client", "command":"acquire",
        "operation_id":"78af4405-b644-44ab-b58d-53ce4d62acc0", "expected_revision":0});
    let request = |body: Value, owner: &str| {
        let mut request = crate::rest::agents_tests::request(
            Method::POST,
            "/api/v1/agents/agt_shared_read/browser/control",
            owner,
            None,
            Some(body),
        );
        request.headers_mut().insert(
            "x-cma-browser-client",
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb".parse().unwrap(),
        );
        request
    };
    let admitted = enabled
        .router
        .oneshot(request(original.clone(), "owner-a"))
        .await
        .unwrap();
    assert_eq!(admitted.status(), StatusCode::OK);
    let accepted: Value =
        serde_json::from_slice(&to_bytes(admitted.into_body(), 65536).await.unwrap()).unwrap();
    let recovered = closed
        .router
        .clone()
        .oneshot(request(original.clone(), "owner-a"))
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    let receipt: Value =
        serde_json::from_slice(&to_bytes(recovered.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(receipt, accepted);
    let forbidden = closed
        .router
        .clone()
        .oneshot(request(original.clone(), "owner-b"))
        .await
        .unwrap();
    assert_eq!(forbidden.status(), StatusCode::UNAUTHORIZED);
    for command in ["acquire", "input", "renew", "return"] {
        let mut body = original.clone();
        body["command"] = json!(command);
        body["operation_id"] = json!(uuid::Uuid::new_v4().to_string());
        body["expected_revision"] = json!(1);
        if command != "acquire" {
            body["lease_token"] = accepted["lease_token"].clone();
        }
        if command == "input" {
            body["input"] = json!({"kind":"text","text":"No new input"});
        }
        let response = closed
            .router
            .clone()
            .oneshot(request(body, "owner-a"))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if command == "return" {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "{command}"
        );
    }
    let mut changed = original;
    changed["command"] = json!("input");
    changed["lease_token"] = accepted["lease_token"].clone();
    changed["input"] = json!({"kind":"text","text":"Changed intent"});
    let response = closed
        .router
        .oneshot(request(changed, "owner-a"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
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
