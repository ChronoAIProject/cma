use super::*;

#[test]
fn every_trigger_launch_and_connection_operation_is_documented() {
    let document = serde_json::to_value(document()).expect("OpenAPI serializes");
    let expected = [
        ("get", "/auth/signup-options"),
        ("get", "/api/v1/triggers"),
        ("post", "/api/v1/triggers"),
        ("get", "/api/v1/triggers/{id}"),
        ("put", "/api/v1/triggers/{id}"),
        ("delete", "/api/v1/triggers/{id}"),
        ("post", "/api/v1/triggers/{id}/publish"),
        ("post", "/api/v1/triggers/{id}/retire"),
        ("get", "/api/v1/triggers/{id}/revisions"),
        ("get", "/api/v1/triggers/{id}/stats"),
        ("get", "/api/v1/triggers/public/{public_id}"),
        ("get", "/api/v1/triggers/public/{public_id}/badge.svg"),
        ("get", "/api/v1/triggers/{id}/embed"),
        ("get", "/api/v1/triggers/{id}/placements"),
        ("post", "/api/v1/triggers/{id}/placements"),
        (
            "post",
            "/api/v1/triggers/{id}/placements/{placement_id}/retire",
        ),
        ("post", "/api/v1/triggers/public/{public_id}/launches"),
        ("post", "/api/v2/triggers/public/{public_id}/launches"),
        ("get", "/api/v1/launches"),
        ("get", "/api/v1/launches/{id}"),
        ("get", "/api/v1/launches/{id}/events"),
        ("get", "/api/v1/launches/{id}/requirements"),
        ("post", "/api/v1/launches/{id}/start"),
        ("post", "/api/v1/launches/{id}/cancel"),
        ("post", "/api/v1/service-connections"),
        ("get", "/api/v1/service-connections/{handoff_id}"),
        ("get", "/api/v1/agui/launches/{id}/events"),
    ];

    for (method, path) in expected {
        assert!(
            !operation(&document, method, path).is_null(),
            "missing {method} {path}"
        );
    }
}

#[test]
fn bound_launch_documents_required_condition_and_terminal_proof() {
    let document = serde_json::to_value(document()).unwrap();
    let route = operation(
        &document,
        "post",
        "/api/v2/triggers/public/{public_id}/launches",
    );
    let condition = route["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "X-CMA-Expected-Subject")
        .unwrap();
    assert_eq!(condition["required"], true);
    for status in ["201", "202", "403", "409", "422", "429", "503"] {
        assert!(route["responses"][status]["headers"]
            .get("X-CMA-Verified-Subject")
            .is_some());
    }
}
