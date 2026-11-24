use super::lifecycle::Provisioner;
use super::*;

#[tokio::test]
async fn task_templates_are_validated_before_the_provisioning_port() {
    let fixture = fixture().await;
    let port = Arc::new(Provisioner::default());
    let router = agent_router(fixture.state.with_lifecycle(port.clone()));
    let cases = [
        (
            json!({"id":"research","version":1,"fields":{}}),
            "task_template_field_required",
        ),
        (
            json!({"id":"research","version":1,"fields":{"topic":" \n "}}),
            "task_template_field_required",
        ),
        (
            json!({"id":"research","version":1,"fields":{"topic":"[topic]"}}),
            "task_template_field_placeholder",
        ),
        (
            json!({"id":"research","version":3,"fields":{"topic":"synthetic"}}),
            "task_template_version_unsupported",
        ),
        (
            json!({"id":"unknown","version":1,"fields":{}}),
            "task_template_id_unknown",
        ),
        (
            json!({"id":"research","version":1,"fields":{"topic":"synthetic","secret":"do not echo"}}),
            "task_template_field_unknown",
        ),
        (
            json!({"id":"research","version":1,"fields":{"topic":"synthetic"}}),
            "task_template_message_mismatch",
        ),
        (
            json!({"id":"research","version":1,"fields":{"topic":"界".repeat(87382)}}),
            "task_template_message_too_large",
        ),
    ];
    for (template, code) in cases {
        let mut call = request(
            Method::POST,
            "/api/v1/agents",
            OWNER,
            None,
            Some(json!({"name":"Task","template":template,"initial_message":"Unmatched text"})),
        );
        call.headers_mut()
            .insert("Idempotency-Key", "template-invalid".parse().unwrap());
        let response = router.clone().oneshot(call).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["code"], code);
        assert!(!body.to_string().contains("do not echo"));
        assert!(port.calls.lock().unwrap().is_empty());
    }
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../layer2-core/src/sandbox/task_template_vectors.json"
    ))
    .unwrap();
    let count = vectors.len();
    for vector in vectors {
        let mut call = request(
            Method::POST,
            "/api/v1/agents",
            OWNER,
            None,
            Some(
                json!({"name":"Task","template":vector["template"],"initial_message":vector["message"]}),
            ),
        );
        call.headers_mut()
            .insert("Idempotency-Key", "valid-template".parse().unwrap());
        assert_eq!(
            router.clone().oneshot(call).await.unwrap().status(),
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(port.calls.lock().unwrap().len(), count);
}
