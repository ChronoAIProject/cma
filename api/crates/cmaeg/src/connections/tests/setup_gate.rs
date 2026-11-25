use super::*;

#[tokio::test]
async fn disabled_setup_hides_capability_and_refuses_new_api_admission_without_mutation() {
    let fixture = Fixture::new().await;
    let mut service = fixture.service();
    service.management.managed_telegram_setup_enabled = false;
    let response =
        crate::http::connections::capabilities(State(service.clone()), owner("alice", false)).await;
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["platforms"], json!([]));
    let request = CreateConnection {
        record_id: uuid::Uuid::new_v4().to_string(),
        event_ref: "unopened-fixture-reference".into(),
        suggested_name: None,
    };
    let refused = service
        .create(
            &owner("alice", false),
            crate::auth::AgentCaller { key_hash: [1; 32] },
            request,
        )
        .await;
    assert!(matches!(
        refused,
        Err(Error::Forbidden("managed_telegram_setup_disabled"))
    ));
    assert!(fixture.peer.rows.lock().unwrap().is_empty());
    assert!(fixture.peer.channels.lock().unwrap().is_empty());
    assert_eq!(fixture.peer.telegram_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.peer.auth_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn disabled_setup_preserves_owned_list_read_and_cancel() {
    let fixture = Fixture::new().await;
    let mut service = fixture.service();
    service.management.managed_telegram_setup_enabled = false;
    let (empty, next) = service.list(&owner("alice", false), None).await.unwrap();
    assert!(empty.is_empty());
    assert!(next.is_none());
    let mut record = connection("alice", false);
    record.status = Status::Ready;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    assert_eq!(
        service
            .get(&owner("alice", false), &record.id)
            .await
            .unwrap()
            .connection_ref,
        record.id
    );
    assert_eq!(
        service
            .list(&owner("alice", false), None)
            .await
            .unwrap()
            .0
            .len(),
        1
    );
    assert!(service
        .list(&owner("bob", false), None)
        .await
        .unwrap()
        .0
        .is_empty());
    service
        .cancel(&owner("alice", false), &record.id)
        .await
        .unwrap();
    assert_eq!(
        service
            .get(&owner("alice", false), &record.id)
            .await
            .unwrap()
            .status,
        Status::Cancelled
    );
    assert_eq!(fixture.peer.telegram_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn enabled_setup_advertises_the_same_methods_as_before() {
    let fixture = Fixture::new().await;
    let response =
        crate::http::connections::capabilities(State(fixture.service()), owner("alice", false))
            .await;
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["platforms"][0]["connection_methods"],
        json!(["existing_credential", "managed_create"])
    );
}

#[tokio::test]
async fn disabled_setup_cannot_be_bypassed_through_the_authenticated_http_route() {
    let fixture = Fixture::new().await;
    let mut service = fixture.service();
    service.management.managed_telegram_setup_enabled = false;
    let app = crate::http::connections::router(service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/channel-connections",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth("fixture-person-bearer")
        .header(
            "x-cmaeg-agent-authorization",
            format!("Bearer nyx_{}", uuid::Uuid::new_v4().simple()),
        )
        .json(&json!({
            "record_id":uuid::Uuid::new_v4().to_string(),"event_ref":"unopened-fixture-reference"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body: Value = response.json().await.unwrap();
    server.abort();
    assert_eq!(body["detail"], "managed_telegram_setup_disabled");
    assert!(fixture.peer.rows.lock().unwrap().is_empty());
    assert_eq!(fixture.peer.telegram_calls.load(Ordering::SeqCst), 0);
}
