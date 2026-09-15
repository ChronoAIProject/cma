use super::fixtures::*;
use axum::{
    body::Body,
    http::{Method, StatusCode},
};
use layer2_core::sandbox::{agent_io::*, store::PageRequest};
use serde_json::json;

#[tokio::test]
async fn response_controls_strict_json_key_and_query_validation_never_admits_work() {
    let f = fixture(true).await;
    let valid = intent(&f, json!({"type":"cancel"})).await;
    for (key, body) in [
        (None, valid.clone()),
        (Some(""), valid.clone()),
        (
            Some("valid"),
            json!({"turn_sequence":0,"expected_agent_revision":1,"command":{"type":"cancel"}}),
        ),
    ] {
        assert_eq!(
            call(&f.router, Method::POST, PATH, "owner-a", key, Some(body))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    for field in [
        "actor",
        "owner",
        "source",
        "credential",
        "runtime_locator",
        "response_id",
    ] {
        let mut body = valid.clone();
        body[field] = json!("forged");
        let (status, problem) = call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("strict"),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(problem["error"]["code"], "invalid_control_request");
    }
    for command in [
        json!({"type":"cancel","text":"extra"}),
        json!({"type":"steer","text":"","actor":"forged"}),
        json!({"type":"message","text":"unsupported"}),
        json!({"type":"resolve_approval","approval_id":"a","decision":"approve"}),
    ] {
        let mut body = valid.clone();
        body["command"] = command;
        assert_eq!(
            call(
                &f.router,
                Method::POST,
                PATH,
                "owner-a",
                Some("strict"),
                Some(body)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut duplicate = request(
        Method::POST,
        PATH,
        "owner-a",
        Some("one"),
        Some(valid.clone()),
    );
    duplicate
        .headers_mut()
        .append("idempotency-key", "two".parse().unwrap());
    assert_eq!(
        crate::rest::agents_tests::json_response(&f.router, duplicate)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut duplicate_json = request(Method::POST, PATH, "owner-a", Some("one"), Some(valid));
    *duplicate_json.body_mut() = Body::from(
        r#"{"turn_sequence":1,"turn_sequence":2,"expected_agent_revision":1,"command":{"type":"cancel"}}"#,
    );
    let (status, problem) =
        crate::rest::agents_tests::json_response(&f.router, duplicate_json).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(problem["error"]["code"], "invalid_control_request");
    for path in [
        format!("{PATH}?actor=forged"),
        format!("/api/v2/commands/{SOURCE}?unexpected=true"),
    ] {
        let (status, problem) = call(&f.router, Method::GET, &path, "owner-a", None, None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(problem["error"]["code"], "controls_field_unsupported");
    }
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn response_control_openapi_requires_key_and_keeps_response_schema_separate() {
    let document = serde_json::to_value(super::super::document()).unwrap();
    let operation =
        &document["paths"]["/api/v2/agents/{agent_id}/responses/{response_id}/controls"]["post"];
    assert!(operation["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "Idempotency-Key" && p["required"] == true));
    assert_eq!(
        operation["responses"]["202"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/V2ResponseControl"
    );
    assert!(
        document["components"]["schemas"]["V2ResponseObject"]["properties"]
            .get("control")
            .is_none()
    );
}
