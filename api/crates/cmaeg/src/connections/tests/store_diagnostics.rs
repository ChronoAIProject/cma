use super::*;
use std::io::Write;
use tracing::instrument::WithSubscriber;

#[path = "router_diagnostics.rs"]
mod router;

const UPSTREAM_PRIVATE: &str = "fixture-private-upstream-body";

#[derive(Clone)]
pub(super) struct ReadFault {
    stage: &'static str,
    status: StatusCode,
    body: String,
    delay: Duration,
}

pub(super) async fn read_fault(peer: &Peer, stage: &str) -> Option<Response> {
    let fault = peer.read_fault.lock().unwrap().clone()?;
    if fault.stage != stage {
        return None;
    }
    tokio::time::sleep(fault.delay).await;
    Some((fault.status, fault.body).into_response())
}

pub(super) async fn deny_auth(State(peer): State<Arc<Peer>>) -> Response {
    peer.auth_calls.fetch_add(1, Ordering::SeqCst);
    (StatusCode::FORBIDDEN, UPSTREAM_PRIVATE).into_response()
}

pub(super) async fn telegram(State(peer): State<Arc<Peer>>) -> Response {
    peer.telegram_calls.fetch_add(1, Ordering::SeqCst);
    StatusCode::BAD_GATEWAY.into_response()
}

pub(super) async fn creator(headers: HeaderMap) -> Response {
    let (subject, kind) = match headers.get("authorization").and_then(|v| v.to_str().ok()) {
        Some("Bearer fixture-person-bearer") => ("fixture-owner-private", "person"),
        Some("Bearer fixture-alice-bearer") => ("alice", "person"),
        Some("Bearer fixture-organization-bearer") => ("fixture-owner-private", "org"),
        Some("Bearer fixture-foreign-bearer") => ("fixture-other-owner", "person"),
        _ => return StatusCode::UNAUTHORIZED.into_response(),
    };
    Json(json!({"subject":subject,"user_type":kind})).into_response()
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Logs {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        static SHARED: std::sync::OnceLock<tracing::Dispatch> = std::sync::OnceLock::new();
        SHARED.get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
        let writer = self.clone();
        tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_ansi(false)
            .with_env_filter(
                "off,cmaeg::connections::store=warn,cmaeg::records=warn,cmaeg::telemetry::http=info",
            )
            .with_writer(move || writer.clone())
            .finish()
    }

    fn events(&self, fixture: &Fixture) -> Vec<Value> {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        for private in [
            CHILD_TOKEN,
            WEBHOOK_SECRET,
            REPLY_TOKEN,
            SERVICE_TOKEN,
            UPSTREAM_PRIVATE,
            "fixture-owner-private",
            "fixture-other-owner",
            &fixture.base,
        ] {
            assert!(
                !text.contains(private),
                "diagnostic exposed private fixture data"
            );
        }
        for (id, row) in fixture.peer.rows.lock().unwrap().iter() {
            assert!(!text.contains(id));
            assert!(!text.contains(&row.blob));
        }
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

async fn list_response(fixture: &Fixture, creator: Creator) -> (StatusCode, Value, Logs) {
    let mut service = fixture.service();
    service.management.telegram = Some(Arc::new(
        crate::telegram::TelegramClient::new(&fixture.base, Duration::from_secs(1)).unwrap(),
    ));
    let logs = Logs::default();
    let response = async {
        crate::http::connections::list(
            State(service),
            creator,
            Query(serde_json::from_value(json!({})).unwrap()),
        )
        .await
    }
    .with_subscriber(logs.subscriber())
    .await;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(fixture.peer.telegram_calls.load(Ordering::SeqCst), 0);
    (status, body, logs)
}

fn assert_failure(
    fixture: &Fixture,
    result: (StatusCode, Value, Logs),
    stage: &str,
    backend: &str,
    class: &str,
    http_status: Option<u16>,
) {
    let (status, body, logs) = result;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let expected = Error::Unavailable.problem();
    assert_eq!(body, serde_json::to_value(expected).unwrap());
    assert_event(fixture, logs, stage, backend, class, http_status);
}

fn assert_event(
    fixture: &Fixture,
    logs: Logs,
    stage: &str,
    backend: &str,
    class: &str,
    http_status: Option<u16>,
) {
    let events = logs.events(fixture);
    assert_eq!(
        events.len(),
        1,
        "one diagnostic per failed operation: {events:?}"
    );
    let mut fields = json!({
        "message": "connection record read failed", "stage": stage,
        "backend": backend, "error_class": class,
    });
    if let Some(status) = http_status {
        fields["http_status"] = status.into();
    }
    assert_eq!(events[0]["fields"], fields);
}

async fn ready(fixture: &Fixture) -> Connection {
    let mut record = connection("fixture-owner-private", false);
    record.status = Status::Ready;
    fixture
        .store()
        .save_connection(&record, None)
        .await
        .unwrap();
    record
}

#[tokio::test]
async fn empty_and_ineligible_pages_keep_the_success_contract() {
    let fixture = Fixture::new().await;
    let creator = owner("fixture-owner-private", false);
    let (status, body, logs) = list_response(&fixture, creator.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));
    assert!(logs.events(&fixture).is_empty());
    for (subject, organization, status, bound) in [
        ("fixture-owner-private", false, Status::Draft, false),
        ("fixture-owner-private", false, Status::Ready, true),
        ("fixture-owner-private", true, Status::Ready, false),
        ("fixture-other-owner", false, Status::Ready, false),
    ] {
        let mut record = connection(subject, organization);
        record.status = status;
        record.channel_record = bound.then(|| uuid::Uuid::new_v4().to_string());
        fixture
            .store()
            .save_connection(&record, None)
            .await
            .unwrap();
    }
    let (status, body, logs) = list_response(&fixture, creator.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], json!([]));
    assert!(logs.events(&fixture).is_empty());
    let record = ready(&fixture).await;
    let (status, body, logs) = list_response(&fixture, creator).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["connection_ref"], record.id);
    assert!(logs.events(&fixture).is_empty());
}

#[tokio::test]
async fn page_and_get_http_failures_have_one_safe_diagnostic() {
    for stage in ["page", "get"] {
        for status in [StatusCode::SERVICE_UNAVAILABLE, StatusCode::FORBIDDEN] {
            let fixture = Fixture::new().await;
            ready(&fixture).await;
            *fixture.peer.read_fault.lock().unwrap() = Some(ReadFault {
                stage,
                status,
                body: UPSTREAM_PRIVATE.into(),
                delay: Duration::ZERO,
            });
            let result = list_response(&fixture, owner("fixture-owner-private", false)).await;
            assert_failure(
                &fixture,
                result,
                stage,
                "legacy_http",
                "http_status",
                Some(status.as_u16()),
            );
        }
    }
}

#[tokio::test]
async fn malformed_response_and_record_payloads_keep_errors_and_distinct_stages() {
    for (stage, body, class) in [
        ("page", UPSTREAM_PRIVATE.to_owned(), "response_decode"),
        ("get", UPSTREAM_PRIVATE.to_owned(), "response_decode"),
        (
            "get",
            json!({"blob":"%%%", "version":1}).to_string(),
            "response_decode",
        ),
        ("decrypt", STANDARD.encode(UPSTREAM_PRIVATE), "envelope"),
        ("decode", UPSTREAM_PRIVATE.into(), "record_decode"),
        (
            "decode",
            json!({"record_kind":"unknown", "private":UPSTREAM_PRIVATE}).to_string(),
            "record_decode",
        ),
        ("page", UPSTREAM_PRIVATE.repeat(16_384), "response_decode"),
        (
            "get",
            json!({"blob":"", "version":1}).to_string(),
            "response_decode",
        ),
    ] {
        let fixture = Fixture::new().await;
        let record = ready(&fixture).await;
        // A second readable record must not turn a corrupt page into partial success.
        ready(&fixture).await;
        if stage == "decrypt" || stage == "decode" {
            let blob = if stage == "decode" {
                STANDARD.encode(
                    cmaeg_core::seal::seal_connection(&fixture.keys, &record.id, body.as_bytes())
                        .unwrap(),
                )
            } else {
                body
            };
            fixture
                .peer
                .rows
                .lock()
                .unwrap()
                .get_mut(&record.id)
                .unwrap()
                .blob = blob;
        } else {
            *fixture.peer.read_fault.lock().unwrap() = Some(ReadFault {
                stage,
                status: StatusCode::OK,
                body,
                delay: Duration::ZERO,
            });
        }
        let result = list_response(&fixture, owner("fixture-owner-private", false)).await;
        assert_failure(&fixture, result, stage, "legacy_http", class, None);
    }
}

#[tokio::test]
async fn service_auth_refusal_is_distinct_from_record_http_status() {
    let mut fixture = Fixture::new().await;
    fixture.records_override = Some(
        fixture.records().with_service_auth(
            crate::service_auth::ServiceAuth::account(
                &fixture.base,
                crate::config::ServiceAccountConfig {
                    client_id: "fixture-client".into(),
                    client_secret: Secret::new(SERVICE_TOKEN),
                },
            )
            .unwrap(),
        ),
    );
    let result = list_response(&fixture, owner("fixture-owner-private", false)).await;
    assert_eq!(fixture.peer.auth_calls.load(Ordering::SeqCst), 1);
    assert_failure(
        &fixture,
        result,
        "page",
        "legacy_http",
        "service_auth",
        None,
    );
}

#[tokio::test]
async fn get_timeout_is_classified_without_request_details() {
    let fixture = Fixture::new().await;
    ready(&fixture).await;
    *fixture.peer.read_fault.lock().unwrap() = Some(ReadFault {
        stage: "get",
        status: StatusCode::OK,
        body: UPSTREAM_PRIVATE.into(),
        delay: Duration::from_secs(3),
    });
    let result = list_response(&fixture, owner("fixture-owner-private", false)).await;
    assert_failure(&fixture, result, "get", "legacy_http", "timeout", None);
}

#[tokio::test]
async fn transport_failure_does_not_log_the_request_or_upstream_bytes() {
    use tokio::io::AsyncWriteExt;
    let mut fixture = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    fixture.records_override = Some(RecordClient::new(&base, SERVICE_TOKEN).unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket.write_all(UPSTREAM_PRIVATE.as_bytes()).await.unwrap();
    });
    let result = list_response(&fixture, owner("fixture-owner-private", false)).await;
    server.await.unwrap();
    assert_failure(&fixture, result, "page", "legacy_http", "transport", None);
}

#[tokio::test]
async fn mongo_driver_failure_uses_actual_backend_without_legacy_fallback() {
    let mut fixture = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let options = mongodb::options::ClientOptions::builder()
        .hosts(vec![mongodb::options::ServerAddress::Tcp {
            host: "127.0.0.1".into(),
            port: Some(listener.local_addr().unwrap().port()),
        }])
        .connect_timeout(Duration::from_millis(50))
        .server_selection_timeout(Duration::from_millis(50))
        .build();
    let client = mongodb::Client::with_options(options).unwrap();
    let database = crate::records::mongo::MongoRecords::read_failure_fixture(
        client.database("read_failure_fixture"),
    );
    fixture.records_override = Some(fixture.records().with_database(database));
    let result = list_response(&fixture, owner("fixture-owner-private", false)).await;
    assert_failure(&fixture, result, "page", "mongo", "mongo_read", None);
    let logs = Logs::default();
    let result = async {
        fixture
            .store()
            .load(&uuid::Uuid::new_v4().to_string())
            .await
    }
    .with_subscriber(logs.subscriber())
    .await;
    assert!(matches!(result, Err(Error::Unavailable)));
    assert_event(&fixture, logs, "get", "mongo", "mongo_read", None);
    assert_eq!(fixture.peer.auth_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.peer.telegram_calls.load(Ordering::SeqCst), 0);
    client.shutdown().await;
}

#[tokio::test]
async fn another_stores_records_in_the_connection_kind_do_not_fail_the_list() {
    let fixture = Fixture::new().await;
    let foreign = ready(&fixture).await;
    let kept = ready(&fixture).await;
    // Discord inbox, reaction and fence state share this record kind and owner,
    // sealed the same way but without the connection store's tag.
    let blob = STANDARD.encode(
        cmaeg_core::seal::seal_connection(
            &fixture.keys,
            &foreign.id,
            json!({"inbox": {"message_id": "m1"}, "retained_until": 1})
                .to_string()
                .as_bytes(),
        )
        .unwrap(),
    );
    fixture
        .peer
        .rows
        .lock()
        .unwrap()
        .get_mut(&foreign.id)
        .unwrap()
        .blob = blob;
    let (status, body, logs) = list_response(&fixture, owner("fixture-owner-private", false)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let refs = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["connection_ref"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(refs, vec![kept.id.clone()]);
    assert!(
        logs.events(&fixture).is_empty(),
        "foreign records are not decode failures"
    );
}
