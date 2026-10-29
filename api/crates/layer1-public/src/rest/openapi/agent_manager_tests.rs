use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

#[path = "agent_manager_route_tests.rs"]
mod routes;

fn full() -> Value {
    serde_json::to_value(super::super::document()).unwrap()
}

#[test]
fn focused_contract_preserves_existing_operations_and_identities() {
    let full = full();
    let focused = document();
    let mut ids = BTreeSet::new();
    let mut operation_count = 0;
    for (path, methods) in OPERATIONS {
        for method in *methods {
            assert_eq!(
                focused["paths"][path][method], full["paths"][path][method],
                "{method} {path}"
            );
            assert!(ids.insert(
                focused["paths"][path][method]["operationId"]
                    .as_str()
                    .unwrap()
            ));
            operation_count += 1;
        }
    }
    assert_eq!(focused["paths"].as_object().unwrap().len(), 62);
    assert_eq!(operation_count, 76);
    assert_eq!(focused["servers"][0]["url"], "/");
    assert_eq!(focused["info"]["title"], "CMA Agent Manager API");
    assert!(focused["info"]["description"]
        .as_str()
        .unwrap()
        .contains("Idempotency-Key"));
    // Preserve the exact code-derived component shapes, including unconstrained
    // JSON schemas that Utoipa serializes but cannot itself deserialize.
    for (kind, values) in focused["components"].as_object().unwrap() {
        for (name, value) in values.as_object().unwrap() {
            assert_eq!(value, &full["components"][kind][name], "{kind}/{name}");
        }
    }
}

#[test]
fn focused_contract_has_one_creation_and_interaction_model() {
    let full = full();
    let focused = document();
    for path in [
        "/api/v1/agent-profiles",
        "/api/v1/marketplace/{kind}",
        "/api/v1/admin/settings",
        "/api/v1/agents/{id}/input",
        "/api/v1/agents/{id}/interrupt",
        "/api/v1/agents/{id}/steer",
        "/api/v1/responses",
        "/api/v2/responses",
        "/api/v1/workspaces/{id}/agents",
    ] {
        assert!(
            focused["paths"].get(path).is_none(),
            "unfocused path {path}"
        );
    }
    for (path, method) in [
        ("/api/v1/sandboxes", "post"),
        ("/api/v1/sandboxes/{id}/workspaces", "post"),
        ("/api/v1/agents/{id}/artifacts", "post"),
    ] {
        assert!(full["paths"][path].get(method).is_some());
        assert!(focused["paths"][path].get(method).is_none());
    }
    for field in ["workspace_id", "sandbox_id"] {
        assert!(
            focused["components"]["schemas"]["AgentLifecycleCreateRequest"]["properties"]
                .get(field)
                .is_some()
        );
    }
    assert!(full["paths"].get("/api/v1/agent-profiles").is_some());
    assert!(full["paths"].get("/api/v1/responses").is_some());
}

fn assert_references_resolve(value: &Value, document: &Value) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                assert!(
                    reference.starts_with('#'),
                    "unexpected external reference {reference}"
                );
                assert!(
                    document.pointer(&reference[1..]).is_some(),
                    "dangling {reference}"
                );
            }
            for value in object.values() {
                assert_references_resolve(value, document);
            }
        }
        Value::Array(values) => {
            for value in values {
                assert_references_resolve(value, document);
            }
        }
        _ => {}
    }
}

#[test]
fn pruning_removes_all_unreachable_components_and_tags_without_dangling_references() {
    let full = full();
    let mut focused = document();
    assert_references_resolve(&focused, &focused);
    let original = focused.clone();
    prune_components(&mut focused);
    assert_eq!(focused, original);
    let components = focused["components"].as_object().unwrap();
    let mut roots = focused.clone();
    roots.as_object_mut().unwrap().remove("components");
    let mut reachable = BTreeSet::new();
    dependencies(&roots, &mut reachable);
    loop {
        let before = reachable.len();
        for (kind, name) in reachable.clone() {
            dependencies(&components[&kind][&name], &mut reachable);
        }
        if reachable.len() == before {
            break;
        }
    }
    let retained: BTreeSet<_> = components
        .iter()
        .flat_map(|(kind, values)| {
            values
                .as_object()
                .unwrap()
                .keys()
                .map(move |name| (kind.clone(), name.clone()))
        })
        .collect();
    assert_eq!(retained, reachable);
    for name in [
        "ProfilePageView",
        "PlatformSettingsBody",
        "GatewayRecord",
        "CreateResponseRequest",
    ] {
        assert!(
            focused["components"]["schemas"].get(name).is_none(),
            "unrelated schema {name}"
        );
    }
    let used_tags: BTreeSet<_> = focused["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|item| METHODS.iter().filter_map(move |method| item.get(*method)))
        .flat_map(|operation| operation["tags"].as_array().unwrap())
        .map(|name| name.as_str().unwrap().to_owned())
        .collect();
    let declared_tags: BTreeSet<_> = focused["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tag| tag["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(used_tags, declared_tags);
    let focused_bytes = serde_json::to_vec(&focused).unwrap().len();
    let full_bytes = serde_json::to_vec(&full).unwrap().len();
    assert!(
        focused_bytes * 4 < full_bytes * 3,
        "projection must materially shrink the full contract"
    );
    println!(
        "Agent Manager contract: paths={}, operations=45, schemas={}, bytes={focused_bytes}; full paths={}, schemas={}, bytes={full_bytes}",
        focused["paths"].as_object().unwrap().len(),
        focused["components"]["schemas"].as_object().unwrap().len(),
        full["paths"].as_object().unwrap().len(),
        full["components"]["schemas"].as_object().unwrap().len()
    );
}

#[test]
fn component_closure_keeps_discriminators_cycles_and_only_referenced_security() {
    let mut fixture = json!({
        "paths": {"/a": {"get": {"security": [{"token": []}], "responses": {"200": {"$ref": "#/components/responses/R"}}}}},
        "components": {
            "responses": {"R": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/A"}}}}, "unused": {}},
            "schemas": {"A": {"discriminator": {"mapping": {"b": "B", "c": "#/components/schemas/C"}}},
                "B": {"$ref": "#/components/schemas/A"}, "C": {"$ref": "#/components/schemas/escaped~1name"},
                "escaped/name": {"type": "string"}, "unused": {}},
            "securitySchemes": {"token": {"type": "http", "scheme": "bearer"}, "unused": {}}
        }
    });
    prune_components(&mut fixture);
    assert_eq!(
        fixture["components"]["schemas"].as_object().unwrap().len(),
        4
    );
    assert_eq!(
        fixture["components"]["securitySchemes"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fixture["components"]["responses"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert_references_resolve(&fixture, &fixture);
}

#[test]
fn streaming_controls_queue_and_errors_keep_the_original_wire_contract() {
    let focused = document();
    let schemas = &focused["components"]["schemas"];
    for schema in [
        "CreateV2ResponseRequest",
        "V2ResponseObject",
        "V2ResponseControlRequest",
        "V2ResponseControlCommand",
        "V2CommandStatus",
        "V2QueuedInputRequest",
        "V2QueuedInputPage",
        "V2QueuedCancellationRequest",
        "V2InterruptRequest",
        "ProblemDocument",
        "EventPage",
    ] {
        assert!(schemas.get(schema).is_some(), "missing {schema}");
    }
    let send = &focused["paths"]["/api/v2/agents/{agent_id}/responses"]["post"];
    assert!(send["responses"]["200"]["content"]
        .get("text/event-stream")
        .is_some());
    for suffix in [
        "controls",
        "interrupt",
        "queued-input",
        "queued-cancellation",
    ] {
        let path = format!("/api/v2/agents/{{agent_id}}/responses/{{response_id}}/{suffix}");
        let operation = &focused["paths"][&path]["post"];
        assert!(operation["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "Idempotency-Key" && p["required"] == true));
        for status in ["401", "403", "404", "409", "422", "503"] {
            assert!(
                operation["responses"].get(status).is_some(),
                "{path}: missing {status}"
            );
        }
    }
}

#[tokio::test]
async fn canonical_discovery_is_public_json_and_full_document_remains_unchanged() {
    for (path, expected) in [
        ("/agent-manager/openapi.json", document()),
        ("/openapi.json", full()),
    ] {
        let response = crate::rest::router()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/json");
        let actual: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(actual, expected);
    }
}
