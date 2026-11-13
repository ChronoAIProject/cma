use super::*;

#[tokio::test]
async fn canonical_alias_disconnect_and_retrieval_keep_one_durable_receipt() {
    let f = fixture().await;
    let before = f
        .store
        .get_agent(&AgentId("agt_fixture".into()))
        .await
        .unwrap();
    let mut hostile = request(
        Method::POST,
        "/api/v2/responses",
        "owner-a",
        Some("same-key"),
        Some(
            json!({"conversation":"agt_fixture","input":"Hello","stream":true,"metadata":{"private":"owner metadata"}}),
        ),
    );
    hostile
        .headers_mut()
        .insert("host", "evil.example".parse().unwrap());
    hostile
        .headers_mut()
        .insert("x-forwarded-host", "evil.example".parse().unwrap());
    let created = f.router.clone().oneshot(hostile).await.unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let id = created.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let command = created.headers()["x-cma-command-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let mut body = created.into_body().into_data_stream();
    let first = events(&body.next().await.unwrap().unwrap());
    assert_eq!(first[0]["type"], "response.created");
    assert_eq!(first[0]["response"]["status"], "queued");
    assert_eq!(first[0]["response"]["id"], id);
    let callback = first[0]["response"]["callback_url"].as_str().unwrap();
    assert_eq!(callback, "https://cma.example/agents/agt_fixture");
    assert!(!callback.contains("resp_"));
    drop(body);
    let retry = create(&f, true).await;
    assert_eq!(retry.status(), StatusCode::OK);
    assert_eq!(retry.headers()["x-cma-response-id"], id);
    assert_eq!(retry.headers()["x-cma-command-id"], command);
    drop(retry);
    let (status, recovered) = read_json(
        &f.router,
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "owner-a",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovered, first[0]["response"]);
    assert_eq!(
        f.store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        f.store
            .get_agent(&AgentId("agt_fixture".into()))
            .await
            .unwrap(),
        before
    );
    let (status, conflict) = read_json(
        &f.router,
        Method::POST,
        "/api/v2/responses",
        "owner-a",
        Some(json!({"conversation":"agt_fixture","input":"changed","stream":true})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "agent_command_key_conflict");
}

#[tokio::test]
async fn field_validation_is_named_and_never_admits_work() {
    let f = fixture().await;
    for (field, value, code) in [
        ("stream", json!(false), "stream_required"),
        (
            "previous_response_id",
            json!("resp_previous"),
            "conversation_previous_response_conflict",
        ),
        ("model", json!("different-model"), "model_mismatch"),
        ("store", json!(false), "store_required"),
        ("tools", json!([]), "responses_field_unsupported"),
        ("instructions", Value::Null, "responses_field_unsupported"),
        ("reasoning", json!({}), "responses_field_unsupported"),
        ("background", json!(true), "responses_field_unsupported"),
        ("actor", json!("owner-a"), "responses_field_unsupported"),
        ("metadata", json!({"bad":2}), "invalid_metadata"),
    ] {
        let mut body = json!({"conversation":"agt_fixture","stream":true,"input":"Hello"});
        body[field] = value;
        let (status, refusal) = read_json(
            &f.router,
            Method::POST,
            "/api/v2/responses",
            "owner-a",
            Some(body),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field}: {refusal}"
        );
        assert_eq!(refusal["error"]["code"], code);
        assert_eq!(refusal["error"]["param"], field);
    }
    let (status, refusal) = read_json(
        &f.router,
        Method::POST,
        "/api/v2/agents/agt_other/responses",
        "owner-a",
        Some(json!({"conversation":"agt_fixture","stream":true,"input":"Hello"})),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(refusal["error"]["code"], "conversation_alias_mismatch");
    assert!(f
        .store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn forged_response_ids_and_inaccessible_receipts_have_identical_not_found_errors() {
    let f = fixture().await;
    let response = create(&f, false).await;
    let id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    drop(response);
    let (status, hidden) = read_json(
        &f.router,
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "unrelated",
        None,
    )
    .await;
    let (missing_status, missing) = read_json(
        &f.router,
        Method::GET,
        "/api/v2/responses/resp_missing",
        "unrelated",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing_status, status);
    assert_eq!(missing, hidden);
    let mut forged = request(
        Method::GET,
        &format!("/api/v2/responses/{id}"),
        "unrelated",
        None,
        None,
    );
    forged.extensions_mut().insert(context("owner-a"));
    let (status, value) = fixtures::json_response(&f.router, forged).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(value["error"]["code"], "authentication_required");
}
