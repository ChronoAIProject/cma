use super::*;

fn scope_request(key: Option<&str>, body: Value) -> Request<Body> {
    let mut value = request(Method::PUT, None, key, Some(body));
    *value.uri_mut() = "/api/v1/agents/agent-one/service-access".parse().unwrap();
    value
}

fn selection() -> Value {
    json!({"services":["chrono-llm-public","ornn-api"],"expected_services":["chrono-llm-public","ornn-api","api-github"]})
}

#[tokio::test]
async fn owner_save_forwards_exact_removal_and_returns_durable_accepted_receipt() {
    let (router, service) = app();
    for _ in 0..2 {
        let (status, value) =
            agents_tests::json_response(&router, scope_request(Some("owner-save"), selection()))
                .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(value["status"], "connecting");
        assert_eq!(
            value["connection"]["target"]["type"],
            "agent_access_request"
        );
    }
    let calls = service.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
    assert_eq!(
        calls[0],
        json!({"action":"replace","caller":"owner","agent":"agent-one","services":selection()["services"],"expected_services":selection()["expected_services"],"key":"owner-save"})
    );
}

#[tokio::test]
async fn owner_save_rejects_missing_authority_key_duplicates_and_unknown_fields_before_effects() {
    let (router, service) = app();
    for case in 0..7 {
        let mut value = selection();
        let mut key = Some("owner-save");
        match case {
            0 => key = None,
            1 => value["services"] = json!(["ornn-api", "ornn-api"]),
            2 => value["expected_services"] = json!(["ornn-api", "ornn-api"]),
            3 => value["unexpected"] = true.into(),
            _ => (),
        }
        let mut request = scope_request(key, value);
        match case {
            4 => {
                request
                    .extensions_mut()
                    .remove::<crate::AuthenticatedSubject>();
            }
            5 => {
                request.extensions_mut().insert(context("foreign"));
            }
            6 => {
                request
                    .extensions_mut()
                    .remove::<crate::DelegatedCredential>();
            }
            _ => (),
        }
        let expected = match case {
            3 => 422,
            4 | 5 => 401,
            6 => 403,
            _ => 400,
        };
        assert_eq!(
            router
                .clone()
                .oneshot(request)
                .await
                .unwrap()
                .status()
                .as_u16(),
            expected
        );
    }
    assert!(service.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn owner_save_cas_conflict_and_unavailable_keep_public_problem_semantics() {
    let (router, service) = app();
    for (error, expected) in [
        (AgentAccessError::Conflict, 409),
        (AgentAccessError::Unavailable, 503),
    ] {
        *service.error.lock().unwrap() = Some(error);
        let response = router
            .clone()
            .oneshot(scope_request(Some("owner-save"), selection()))
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
    }
}

#[test]
fn owner_save_openapi_requires_expected_scope_and_idempotency_key() {
    let value = serde_json::to_value(super::super::openapi::document()).unwrap();
    let operation = &value["paths"]["/api/v1/agents/{id}/service-access"]["put"];
    assert_eq!(
        operation["operationId"],
        "put_api_v1_agents_by_id_service_access"
    );
    assert!(operation["responses"]["202"].is_object());
    assert!(operation["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["name"] == "Idempotency-Key" && p["required"] == true));
    let required = value["components"]["schemas"]["ReplaceAgentServiceAccess"]["required"]
        .as_array()
        .unwrap();
    assert!(
        required.contains(&json!("services")) && required.contains(&json!("expected_services"))
    );
}
