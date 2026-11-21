use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tower::ServiceExt;

async fn request(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    owner: Option<&str>,
    permitted: bool,
) -> (u16, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("idempotency-key", "rest-key")
        .body(
            body.map(|b| Body::from(b.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap();
    if let Some(owner) = owner {
        request
            .extensions_mut()
            .insert(layer1_public::AuthenticatedSubject(SubjectId::new(owner)));
        request
            .extensions_mut()
            .insert(layer2_core::Authorization::new(if permitted {
                vec![layer2_core::Capability::ManageOwnBots]
            } else {
                vec![]
            }));
    }
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), 1_048_576).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn bot_rest_admits_durable_work_and_hides_other_owners_without_exposing_keys() {
    let server = Server::start().await;
    let gateway = Gateway::start(&server).await;
    let (service, _, worker, initial) = lifecycle::setup(&server, &gateway).await;
    let router = layer1_public::rest::bot_router(BotControl::new(
        service,
        lifecycle::external(&server, &gateway),
    ));
    let body = json!({"name":"REST bot","nyxid_bot_id":"external-bot", "profile":{"id":"agp_fixture","revision":1},
        "admission":{"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"deny"},"reply":{"auto_final":true,"ack_after_ms":0}});
    assert_eq!(
        request(
            &router,
            "POST",
            "/api/v1/bots",
            Some(body.clone()),
            None,
            false
        )
        .await
        .0,
        401
    );
    assert_eq!(
        request(
            &router,
            "POST",
            "/api/v1/bots",
            Some(body.clone()),
            Some("owner"),
            false
        )
        .await
        .0,
        403
    );
    let mut unknown = body.clone();
    unknown["admission"] = json!({"type":"all","unexpected":true});
    assert_eq!(
        request(
            &router,
            "POST",
            "/api/v1/bots",
            Some(unknown),
            Some("owner"),
            true
        )
        .await
        .0,
        422
    );
    let (status, created) = request(
        &router,
        "POST",
        "/api/v1/bots",
        Some(body.clone()),
        Some("owner"),
        true,
    )
    .await;
    assert_eq!(status, 201);
    assert_eq!(created["state"], "creating");
    let (_, replay) = request(
        &router,
        "POST",
        "/api/v1/bots",
        Some(body.clone()),
        Some("owner"),
        true,
    )
    .await;
    assert_eq!(created["bot_id"], replay["bot_id"]);
    let mut changed = body;
    changed["name"] = "different request".into();
    assert_eq!(
        request(
            &router,
            "POST",
            "/api/v1/bots",
            Some(changed),
            Some("owner"),
            true
        )
        .await
        .0,
        409
    );
    let id = created["bot_id"].as_str().unwrap();
    let path = format!("/api/v1/bots/{id}");
    assert_eq!(
        request(&router, "GET", &path, None, Some("other"), true)
            .await
            .0,
        404
    );
    worker.run(id).await.unwrap();
    let (_, active) = request(&router, "GET", &path, None, Some("owner"), true).await;
    assert_eq!(active["state"], "active");
    let (status, result) = request(
        &router,
        "POST",
        &format!("{path}/test"),
        None,
        Some("owner"),
        true,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(result["first_text"], "Ready");
    for field in [
        "resources",
        "profiles",
        "creation_key_hash",
        "request_fingerprint",
        "credentials",
    ] {
        assert!(active.get(field).is_none());
    }
    assert_ne!(id, initial.bot_id);
    for suffix in ["disable", "enable", "rotate-keys"] {
        assert_eq!(
            request(
                &router,
                "POST",
                &format!("{path}/{suffix}"),
                None,
                Some("owner"),
                true
            )
            .await
            .0,
            202
        );
        worker.run(id).await.unwrap();
    }
    assert_eq!(
        request(&router, "DELETE", &path, None, Some("owner"), true)
            .await
            .0,
        202
    );
    worker.run(id).await.unwrap();
    assert_eq!(
        request(&router, "GET", &path, None, Some("owner"), true)
            .await
            .0,
        404
    );
}
