use super::super::controls::fixtures::{call, fixture, SOURCE};
use axum::http::{Method, StatusCode};
use layer2_core::{ids::AgentCommandId, sandbox::agent_io::AgentCommandStore};
use serde_json::{json, Value};
const LIST: &str = "/api/v2/agents/agt_fixture/queued-inputs";
const EDIT: &str = "/api/v2/agents/agt_fixture/responses/resp_control_source/queued-input";
#[tokio::test]
async fn queued_input_http_lists_and_recovers_exact_edit_after_lost_reply() {
    let f = fixture(false).await;
    let (status, before) = call(&f.router, Method::GET, LIST, "owner-a", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(before["data"].as_array().unwrap().len(), 1);
    let original = f
        .store
        .get_command(&AgentCommandId(SOURCE.into()))
        .await
        .unwrap()
        .unwrap();
    let intent = json!({"source_command_id":SOURCE, "expected_command_revision":before["data"][0]["command_revision"], "text":"revised text\n<attachment>new</attachment>"});
    let (status, accepted) = call(
        &f.router,
        Method::POST,
        EDIT,
        "owner-a",
        Some("edit-one"),
        Some(intent.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(accepted["edit"]["status"], "completed");
    assert_eq!(accepted["response"]["status"], "queued");
    assert_eq!(
        accepted["response"]["receipt"],
        before["data"][0]["response"]["receipt"]
    );
    let (_, replay) = call(
        &f.router,
        Method::POST,
        EDIT,
        "owner-a",
        Some("edit-one"),
        Some(intent.clone()),
    )
    .await;
    assert_eq!(accepted, replay);
    let (_, after) = call(&f.router, Method::GET, LIST, "owner-a", None, None).await;
    assert_eq!(after["data"][0]["text"], intent["text"]);
    assert_eq!(
        after["data"][0]["command_revision"],
        accepted["command_revision"]
    );
    let current = f
        .store
        .get_command(&AgentCommandId(SOURCE.into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.receipt, original.receipt);
    let (status, stale) = call(
        &f.router,
        Method::POST,
        EDIT,
        "owner-a",
        Some("edit-two"),
        Some(intent),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["error"]["code"], "queued_input_not_admitted");
}
#[tokio::test]
async fn queued_input_http_checks_current_authority_and_strict_body() {
    let f = fixture(false).await;
    for route in [LIST, EDIT] {
        let method = if route == LIST {
            Method::GET
        } else {
            Method::POST
        };
        let (status, _) = call(
            &f.router,
            method,
            route,
            "other-owner",
            Some("edit-key"),
            if route == EDIT {
                Some(json!({"source_command_id":SOURCE,"expected_command_revision":1,"text":"x"}))
            } else {
                None
            },
        )
        .await;
        assert!(matches!(
            status,
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ));
    }
    for text in [Value::String(String::new()), Value::Null, json!(" ")] {
        let (status, _) = call(
            &f.router,
            Method::POST,
            EDIT,
            "owner-a",
            Some("invalid"),
            Some(json!({"source_command_id":SOURCE,"expected_command_revision":1,"text":text})),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
    let (status, _) = call(&f.router, Method::POST, EDIT, "owner-a", Some("extra"), Some(json!({"source_command_id":SOURCE,"expected_command_revision":1,"text":"x","actor":"forged"}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
