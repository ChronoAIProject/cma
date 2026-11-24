use super::*;
use sha2::{Digest, Sha256};
use tower_http::{catch_panic::CatchPanicLayer, limit::RequestBodyLimitLayer};

struct App {
    base: String,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl App {
    async fn new(fixture: &Fixture) -> Self {
        let mut service = fixture.service();
        service.management.telegram = Some(Arc::new(
            crate::telegram::TelegramClient::new(&fixture.base, Duration::from_secs(1)).unwrap(),
        ));
        // Match serve.rs: connection routes are merged after router_with_state's telemetry.
        let router = crate::router_with_state(service.management.clone())
            .merge(crate::http::connections::router(service))
            .layer(RequestBodyLimitLayer::new(crate::REQUEST_BODY_LIMIT_BYTES))
            .layer(CatchPanicLayer::custom(
                crate::http::problems::panic_response,
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { base, server }
    }
}

fn storage_events(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|event| {
            event["target"] == "cmaeg::connections::store"
                && event["fields"]["message"] == "connection record read failed"
        })
        .collect()
}

fn assert_private(events: &[Value], app: &App, ids: &[String]) {
    let text = serde_json::to_string(events).unwrap();
    for private in [
        "fixture-person-bearer",
        "fixture-organization-bearer",
        "fixture-foreign-bearer",
        "fixture-invalid-bearer",
        "fixture-private-request-id",
        &app.base,
    ]
    .into_iter()
    .chain(ids.iter().map(String::as_str))
    {
        assert!(
            !text.contains(private),
            "HTTP telemetry exposed private request data"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn actual_connection_router_correlates_concurrent_page_and_get_failures() {
    for (stage, item_route) in [("page", false), ("get", false), ("get", true)] {
        let fixture = Fixture::new().await;
        let record = ready(&fixture).await;
        let retained = fixture.peer.rows.lock().unwrap().clone();
        fixture.peer.requests.lock().unwrap().clear();
        *fixture.peer.read_fault.lock().unwrap() = Some(ReadFault {
            stage,
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: UPSTREAM_PRIVATE.into(),
            delay: Duration::from_millis(10),
        });
        let logs = Logs::default();
        let _capture = tracing::subscriber::set_default(logs.subscriber());
        let app = App::new(&fixture).await;
        let (path, route) = if item_route {
            (
                format!("/v1/channel-connections/{}", record.id),
                "/v1/channel-connections/{connection_ref}",
            )
        } else {
            ("/v1/channel-connections".into(), "/v1/channel-connections")
        };
        let client = reqwest::Client::new();
        let request = || {
            client
                .get(format!("{}{path}", app.base))
                .bearer_auth("fixture-person-bearer")
        };
        let (supplied, generated) = tokio::join!(
            request()
                .header("x-request-id", "fixture-private-request-id")
                .send(),
            request().send(),
        );
        let mut ids = Vec::new();
        for response in [supplied.unwrap(), generated.unwrap()] {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.headers()["cache-control"], "no-store");
            assert_eq!(
                response.headers()["content-type"],
                "application/problem+json"
            );
            let document: Value = serde_json::from_str(&crate::openapi::snapshot()).unwrap();
            let media = response.headers()["content-type"].to_str().unwrap();
            let spec_path = route.strip_prefix("/v1").unwrap();
            assert_eq!(
                document["paths"][spec_path]["get"]["responses"]["default"]["content"][media]
                    ["schema"]["$ref"],
                "#/components/schemas/Problem"
            );
            assert_eq!(response.headers()["cmaeg-protocol-version"], "1");
            let id = response.headers().get("x-request-id").cloned();
            assert_eq!(
                response.json::<Value>().await.unwrap(),
                json!({
                    "type":"about:blank", "title":"provider unavailable", "status":503,
                    "code":"provider_unavailable", "detail":"connection setup is temporarily unavailable",
                })
            );
            ids.push(
                id.expect("connection response lacks request correlation")
                    .to_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        assert_eq!(ids[0], "fixture-private-request-id");
        assert!(!ids[1].is_empty());
        assert_ne!(ids[0], ids[1]);
        let events = logs.events(&fixture);
        let storage = storage_events(&events);
        assert_eq!(
            storage.len(),
            2,
            "one storage diagnostic per failing request"
        );
        let finished: Vec<_> = events
            .iter()
            .filter(|event| event["fields"]["message"] == "request finished")
            .collect();
        assert_eq!(finished.len(), 2);
        for id in &ids {
            let hash = format!("{:x}", Sha256::digest(id.as_bytes()));
            let correlated: Vec<_> = storage
                .iter()
                .filter(|event| event["span"]["request_hash"] == hash)
                .collect();
            assert_eq!(correlated.len(), 1);
            assert_eq!(correlated[0]["span"]["name"], "cmaeg.request");
            assert_eq!(correlated[0]["span"]["route"], route);
            assert_eq!(
                correlated[0]["fields"],
                json!({
                    "message":"connection record read failed", "stage":stage,
                    "backend":"legacy_http", "error_class":"http_status", "http_status":503,
                })
            );
            let completed: Vec<_> = finished
                .iter()
                .filter(|event| event["fields"]["request_hash"] == hash)
                .collect();
            assert_eq!(completed.len(), 1);
            assert_eq!(completed[0]["span"]["route"], route);
            assert_eq!(completed[0]["fields"]["status"], 503);
        }
        assert_private(&events, &app, &ids);
        assert_eq!(fixture.peer.telegram_calls.load(Ordering::SeqCst), 0);
        assert!(fixture.peer.requests.lock().unwrap().is_empty());
        assert!(*fixture.peer.rows.lock().unwrap() == retained);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn actual_connection_router_preserves_authentication_and_owner_isolation() {
    let fixture = Fixture::new().await;
    let logs = Logs::default();
    let _capture = tracing::subscriber::set_default(logs.subscriber());
    let app = App::new(&fixture).await;
    let client = reqwest::Client::new();
    let url = format!("{}/v1/channel-connections", app.base);
    let empty = client
        .get(&url)
        .bearer_auth("fixture-person-bearer")
        .send()
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(empty.json::<Value>().await.unwrap()["items"], json!([]));
    let personal = ready(&fixture).await;
    let mut organization = connection("fixture-owner-private", true);
    organization.status = Status::Ready;
    fixture
        .store()
        .save_connection(&organization, None)
        .await
        .unwrap();
    let mut foreign = connection("fixture-other-owner", false);
    foreign.status = Status::Ready;
    fixture
        .store()
        .save_connection(&foreign, None)
        .await
        .unwrap();
    for bound in [false, true] {
        let mut record = connection("fixture-owner-private", false);
        if bound {
            record.status = Status::Ready;
            record.channel_record = Some(uuid::Uuid::new_v4().to_string());
        }
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
    }
    let retained = fixture.peer.rows.lock().unwrap().clone();
    fixture.peer.requests.lock().unwrap().clear();
    for (token, id) in [
        ("fixture-person-bearer", &personal.id),
        ("fixture-organization-bearer", &organization.id),
        ("fixture-foreign-bearer", &foreign.id),
    ] {
        let response = client.get(&url).bearer_auth(token).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.json::<Value>().await.unwrap();
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["connection_ref"], *id);
    }
    for token in ["fixture-organization-bearer", "fixture-foreign-bearer"] {
        let response = client
            .get(format!("{url}/{}", personal.id))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    for token in [None, Some("fixture-invalid-bearer")] {
        let mut request = client.get(&url);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let events = logs.events(&fixture);
    assert!(storage_events(&events).is_empty());
    assert_private(&events, &app, &[]);
    assert_eq!(fixture.peer.telegram_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.peer.requests.lock().unwrap().is_empty());
    assert!(*fixture.peer.rows.lock().unwrap() == retained);
}
