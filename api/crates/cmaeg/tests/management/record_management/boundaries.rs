use super::super::recovery::{problem, record_probe, serve_fixture};
use super::*;
use axum::{http::StatusCode, routing::post, Json, Router};

#[tokio::test]
async fn authentication_and_read_permission_precede_path_validation_and_record_reads() {
    let fixture = Fixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let auth = Router::new().route("/oauth/introspect", post(move |headers: axum::http::HeaderMap| {
        observed.fetch_add(1, Ordering::SeqCst);
        async move {
            let issuer = format!("http://{}", headers["host"].to_str().unwrap());
            Json(json!({"active":true,"sub":"alice","iss":issuer,
                "exp":time::OffsetDateTime::now_utc().unix_timestamp()+3600,
                "token_type":"access","resource":[format!("{issuer}/api/v1/proxy/s/cma-event-gateway")],
                "permissions":["cma-eg:channels:create"]}))
        }
    }));
    let (issuer, auth_stop, auth_task) = serve_fixture(auth).await;
    let mut state = fixture.state();
    state.creator_auth = Arc::new(
        CreatorAuthenticator::new(&issuer).unwrap().with_service(
            cma_service_auth::NyxIdAuth::new(
                &issuer,
                cma_service_auth::Service::EventGateway,
                "gateway-management",
                "gateway-management-fixture".into(),
            )
            .unwrap(),
        ),
    );
    let (base, stop, task) = serve_app(state).await;
    for id in ["not-a-uuid", "12345678-1234-4234-8123-123456789abc"] {
        let url = format!("{base}/v1/channel-records/{id}/management");
        problem(
            Client::new().get(&url).send().await.unwrap(),
            401,
            "not_authenticated",
        )
        .await;
        problem(
            Client::new()
                .get(&url)
                .bearer_auth("create-only")
                .send()
                .await
                .unwrap(),
            403,
            "not_authenticated",
        )
        .await;
    }
    assert!(
        calls.load(Ordering::SeqCst) > 0,
        "real introspection must run"
    );
    assert_eq!(fixture.cma.gets.load(Ordering::SeqCst), 0);
    stop.send(()).unwrap();
    task.await.unwrap();
    auth_stop.send(()).unwrap();
    auth_task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn malformed_record_ids_are_rejected_before_record_lookup() {
    let fixture = Fixture::new().await;
    let (base, stop, task) = serve_app(fixture.state()).await;
    for id in [
        "not-a-uuid".to_owned(),
        "00000000-0000-0000-0000-000000000000".into(),
        "12345678-1234-1234-8123-123456789abc".into(),
        "12345678-1234-7234-8123-123456789abc".into(),
        "12345678-1234-4234-8123-123456789ABC".into(),
        "12345678123442348123123456789abc".into(),
        "{12345678-1234-4234-8123-123456789abc}".into(),
        "urn:uuid:12345678-1234-4234-8123-123456789abc".into(),
        "x".repeat(513),
        "12345678-1234-4234-8123-123456789abc/extra".into(),
    ] {
        let mut url = reqwest::Url::parse(&format!("{base}/v1/channel-records/")).unwrap();
        url.path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .push(&id)
            .push("management");
        let body = problem(
            Client::new()
                .get(url)
                .bearer_auth("token-alice")
                .send()
                .await
                .unwrap(),
            400,
            "invalid_request",
        )
        .await;
        assert_eq!(body["detail"], "/record_id: must be a canonical UUID v4");
    }
    assert_eq!(fixture.cma.gets.load(Ordering::SeqCst), 0);
    problem(get(&base, &new_id()).await, 404, "channel_not_found").await;
    assert_eq!(fixture.cma.gets.load(Ordering::SeqCst), 1);
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn owner_scope_and_embedded_identity_mismatches_are_opaque() {
    let fixture = Fixture::new().await;
    let (base, stop, task) = serve_app(fixture.state()).await;
    let missing = problem(get(&base, &new_id()).await, 404, "channel_not_found").await;
    for mismatch in ["subject", "organization", "record_id"] {
        let record = definition();
        let mut value = record.to_blob_json();
        match mismatch {
            "subject" => value["owner"]["subject"] = "private-other-owner".into(),
            "organization" => value["owner"]["kind"] = "organization".into(),
            _ => value["record_id"] = new_id().to_string().into(),
        }
        fixture
            .cma
            .seed(record.record_id.as_str(), "alice", sealed_json(&value));
        assert_eq!(
            problem(
                get(&base, &record.record_id).await,
                404,
                "channel_not_found"
            )
            .await,
            missing
        );
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn organization_authority_is_checked_before_path_and_read_and_remains_scoped() {
    let fixture = Fixture::new().await;
    fixture
        .nyxid
        .oauth
        .grant("org-read", "Read organization", "alice", "admin");
    let (alice, _) = fixture.nyxid.oauth.issue_tokens("alice");
    let (bob, _) = fixture.nyxid.oauth.issue_tokens("bob");
    let mut body = create_body();
    body["profile"]["metadata"]["owner"] = json!({"kind":"organization","subject":"org-read"});
    let record = definition_from(body);
    record.validate().unwrap();
    fixture.seed(&record);
    let mut state = fixture.state();
    state.creator_auth = Arc::new(
        CreatorAuthenticator::new(&fixture.nyxid.base_url)
            .unwrap()
            .with_service(
                cma_service_auth::NyxIdAuth::new(
                    &fixture.nyxid.base_url,
                    cma_service_auth::Service::EventGateway,
                    "gateway-management",
                    "gateway-management-fixture".into(),
                )
                .unwrap(),
            ),
    );
    let (base, stop, task) = serve_app(state).await;
    let request = |id: &str, token: &str| {
        Client::new()
            .get(format!("{base}/v1/channel-records/{id}/management"))
            .bearer_auth(token)
            .header("X-CMAEG-Owner", "org-read")
    };
    for id in ["not-a-uuid", record.record_id.as_str()] {
        headers(&request(id, &bob).send().await.unwrap(), 401);
    }
    assert_eq!(fixture.cma.gets.load(Ordering::SeqCst), 0);
    let body = read(
        request(record.record_id.as_str(), &alice)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        body["record"]["owner"],
        json!({"kind":"organization","subject":"org-read"})
    );
    problem(
        get(&base, &record.record_id).await,
        404,
        "channel_not_found",
    )
    .await;
    fixture
        .nyxid
        .oauth
        .grant("org-other", "Other organization", "alice", "admin");
    let response = Client::new()
        .get(url(&base, &record.record_id))
        .bearer_auth(&alice)
        .header("X-CMAEG-Owner", "org-other")
        .send()
        .await
        .unwrap();
    problem(response, 404, "channel_not_found").await;
    let reads = fixture.cma.gets.load(Ordering::SeqCst);
    fixture
        .nyxid
        .oauth
        .grant("org-read", "Read organization", "alice", "member");
    for id in ["not-a-uuid", record.record_id.as_str()] {
        headers(&request(id, &alice).send().await.unwrap(), 401);
    }
    assert_eq!(
        fixture.cma.gets.load(Ordering::SeqCst),
        reads,
        "revoked admin must not read a record"
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn unreadable_blob_and_schema_are_sanitized_errors_even_for_bad_saved_id_types() {
    let fixture = Fixture::new().await;
    let (base, stop, task) = serve_app(fixture.state()).await;
    let record = definition();
    let mut malformed = record.to_blob_json();
    malformed["creation"] = json!({"channel_id":"bad-prefix-private-id","fingerprint":"private-fingerprint","ready":true});
    let mut oversized = malformed.clone();
    oversized["creation"]["channel_id"] = format!("cmaeg1.ch.{}", "x".repeat(513)).into();
    for blob in [
        vec![1, 2, 3],
        sealed_json(&json!({"private-schema":"unknown"})),
        sealed_json(&malformed),
        sealed_json(&oversized),
    ] {
        fixture.cma.seed(record.record_id.as_str(), "alice", blob);
        let response = get(&base, &record.record_id).await;
        headers(&response, 500);
        let body = response.text().await.unwrap();
        assert!(!body.contains("private-"));
        assert!(!body.contains("support-bot"));
        assert!(!body.contains("invalid\""));
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn missing_and_rejected_or_unavailable_store_remain_distinct_and_sanitized() {
    let fixture = Fixture::new().await;
    for (upstream, status, code) in [
        (StatusCode::NOT_FOUND, 404, "channel_not_found"),
        (StatusCode::BAD_REQUEST, 503, "provider_unavailable"),
        (StatusCode::SERVICE_UNAVAILABLE, 503, "provider_unavailable"),
    ] {
        let mut state = fixture.state();
        let (calls, probe_stop, probe_task) = record_probe(&mut state, upstream).await;
        let (base, stop, task) = serve_app(state).await;
        let result = problem(get(&base, &new_id()).await, status, code).await;
        assert!(!result.to_string().contains("private-"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        stop.send(()).unwrap();
        task.await.unwrap();
        probe_stop.send(()).unwrap();
        probe_task.await.unwrap();
    }
    fixture.close().await;
}

#[tokio::test]
async fn management_preserves_reads_and_refuses_unsupported_mutations() {
    let fixture = Fixture::new().await;
    let (base, stop, task) = serve_app(fixture.state()).await;
    for method in [
        reqwest::Method::POST,
        reqwest::Method::PUT,
        reqwest::Method::PATCH,
    ] {
        headers(
            &Client::new()
                .request(method, url(&base, &new_id()))
                .bearer_auth("token-alice")
                .send()
                .await
                .unwrap(),
            405,
        );
    }
    assert_eq!(fixture.cma.gets.load(Ordering::SeqCst), 0);
    let schema: Value = Client::new()
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let operation = &schema["paths"]["/channel-records/{record_id}/management"]["get"];
    assert_eq!(operation["operationId"], "getChannelRecordManagement");
    assert_eq!(operation["security"], json!([{"creatorBearer":[]}]));
    let retirement = &schema["paths"]["/channel-records/{record_id}/management"]["delete"];
    assert_eq!(retirement["operationId"], "retireLegacyChannelRecord");
    assert_eq!(retirement["security"], json!([{"creatorBearer":[]}]));
    let recovery = &schema["paths"]["/channel-records/{record_id}"]["get"];
    assert_eq!(recovery["operationId"], "recoverChannel");
    let required = schema["components"]["schemas"]["ChannelResponse"]["required"]
        .as_array()
        .unwrap();
    for field in ["channel_id", "endpoints"] {
        assert!(required.contains(&json!(field)));
    }
    let management = &schema["components"]["schemas"]["ChannelRecordManagement"];
    assert_eq!(
        management["required"],
        json!(["record_id", "version", "record"])
    );
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn current_personal_owner_subtypes_remain_compatible_and_head_errors_are_no_store() {
    let fixture = Fixture::new().await;
    let (base, stop, task) = serve_app(fixture.state()).await;
    for kind in ["human", "service_account", "agent_key"] {
        let mut body = create_body();
        body["profile"]["metadata"]["owner"]["kind"] = kind.into();
        let mut record = definition_from(body);
        record.owner.kind = record.profile.metadata.owner.kind;
        record.validate().unwrap();
        fixture.seed(&record);
        let response = read(get(&base, &record.record_id).await).await;
        assert_eq!(
            response["record"]["owner"],
            json!({"kind":kind,"subject":"alice"})
        );
    }
    for (token, status) in [(Some("token-alice"), 404), (None, 401)] {
        let mut request = Client::new().head(url(&base, &new_id()));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        headers(&response, status);
        assert!(response.bytes().await.unwrap().is_empty());
    }
    stop.send(()).unwrap();
    task.await.unwrap();
    fixture.close().await;
}
