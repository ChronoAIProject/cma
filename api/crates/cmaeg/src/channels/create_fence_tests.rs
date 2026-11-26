use super::{
    management::ManagementState,
    mutation_fixture::{wait, Fixture},
};
use crate::{auth::CreatorAuthenticator, records::mongo};
use axum::{
    body::{to_bytes, Body},
    extract::Form,
    http::{header, Request, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc},
};
use tower::ServiceExt;

async fn request(router: &Router, method: &str, path: &str, body: Option<&Value>) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, "Bearer fixture-bearer")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = body.and_then(|body| body["record_id"].as_str()) {
        request = request.header("Idempotency-Key", key);
    }
    let body = body.map_or_else(Body::empty, |value| Body::from(value.to_string()));
    router
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn read(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap()
}

fn router(
    mut state: ManagementState,
    db: mongodb::Database,
    issuer: &str,
    service: &cma_service_auth::NyxIdAuth,
    enabled: bool,
) -> Router {
    state.records = Arc::new(
        state
            .records
            .as_ref()
            .clone()
            .with_database(mongo::MongoRecords::read_failure_fixture(db))
            .with_channel_create_fences(issuer, enabled),
    );
    state.creator_auth = Arc::new(
        CreatorAuthenticator::new(issuer)
            .unwrap()
            .with_service(service.clone()),
    );
    crate::http::management::management_router(state)
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn authenticated_cancel_receipt_fences_a_regular_create_across_gateways() {
    let mongo_uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let first = mongodb::Client::with_uri_str(&mongo_uri).await.unwrap();
    let second = mongodb::Client::with_uri_str(&mongo_uri).await.unwrap();
    let name = format!("create_receipt_{}", uuid::Uuid::new_v4().simple());
    let db = first.database(&name);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let claims_issuer = issuer.clone();
    let auth = Router::new().route("/oauth/introspect", post(
        move |Form(body): Form<HashMap<String, String>>| {
            let claims_issuer = claims_issuer.clone();
            async move {
                assert_eq!(body.get("token").map(String::as_str), Some("fixture-bearer"));
                Json(json!({"active":true, "sub":"alice", "iss":claims_issuer,
                    "token_type":"access", "exp": time::OffsetDateTime::now_utc().unix_timestamp() + 60,
                    "permissions":["cma-eg:channels:delete","cma-eg:channels:create"]}))
            }
        },
    )).route("/api/v1/users/me", get(|| async { Json(json!({"id":"alice","user_type":"person"})) }));
    let server = tokio::spawn(async move { axum::serve(listener, auth).await.unwrap() });
    let service = cma_service_auth::NyxIdAuth::new(
        &issuer,
        cma_service_auth::Service::EventGateway,
        "fixture-client",
        "fixture-value".into(),
    )
    .unwrap();
    mongo::migrate(&db).await.unwrap();
    mongo::migrate_create_fence_floor(&db, &issuer)
        .await
        .unwrap();
    let fixture = Fixture::new(false, false).await;
    let capability = "/v1/channel-records/cancel-create-capability";
    let disabled = router(fixture.state().await, db.clone(), &issuer, &service, false);
    assert_eq!(
        request(&disabled, "GET", capability, None).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let gateway_a = router(fixture.state().await, db.clone(), &issuer, &service, true);
    let response = request(&gateway_a, "GET", capability, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read(response).await, json!({"contract_version":1}));
    let body = fixture.body();
    let id = body["record_id"].as_str().unwrap();
    let path = format!("/v1/channel-records/{id}/cancel-create");
    let response = request(&gateway_a, "POST", &path, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let receipt = read(response).await;
    assert_eq!(
        receipt,
        json!({
            "contract_version": 1, "status": "creation_fenced_absent", "record_id": id,
            "owner": {"issuer": issuer, "kind": "human", "subject": "alice"},
            "provider": {"slug":"nyxbot", "binding_id": format!("bnd_{}", id.replace('-', ""))},
        })
    );
    drop(gateway_a);
    let gateway_b = router(
        fixture.state().await,
        second.database(&name),
        &issuer,
        &service,
        true,
    );
    let response = request(&gateway_b, "POST", &path, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(read(response).await, receipt);
    let response = request(&gateway_b, "POST", "/v1/channels", Some(&body)).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(read(response).await["detail"], "channel_create_fenced");
    assert!(fixture.peer.calls.lock().unwrap().is_empty());
    assert!(db
        .collection::<mongodb::bson::Document>(mongo::RECORDS)
        .find_one(mongodb::bson::doc! {"_id": id})
        .await
        .unwrap()
        .is_none());

    let live = fixture.body();
    let response = request(&gateway_b, "POST", "/v1/channels", Some(&live)).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = read(response).await;
    let delete_path = format!(
        "/v1/channels/{}?expected_version={}",
        created["channel_id"].as_str().unwrap(),
        created["version"].as_u64().unwrap()
    );
    let retiring = router(fixture.state().await, db.clone(), &issuer, &service, true);
    fixture.peer.pause.store(true, Ordering::SeqCst);
    let delete =
        tokio::spawn(async move { request(&retiring, "DELETE", &delete_path, None).await });
    wait(&fixture.peer.started).await;
    let (warm, cold) = tokio::join!(
        request(&gateway_b, "POST", "/v1/channels", Some(&live)),
        request(&disabled, "POST", "/v1/channels", Some(&live)),
    );
    assert_eq!(warm.status(), StatusCode::CONFLICT);
    assert_eq!(cold.status(), StatusCode::CONFLICT);
    assert_eq!(*fixture.peer.calls.lock().unwrap(), ["PUT", "DELETE"]);
    fixture.peer.resume.notify_one();
    assert_eq!(delete.await.unwrap().status(), StatusCode::NO_CONTENT);

    db.run_command(mongodb::bson::doc! {"collMod":mongo::RECORDS, "validationAction":"warn"})
        .await
        .unwrap();
    assert_eq!(
        request(&gateway_b, "GET", capability, None).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    db.drop().await.unwrap();
    server.abort();
}
