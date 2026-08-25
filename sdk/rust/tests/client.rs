use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::routing::any;
use cma_sdk::{
    ApprovalDecision, Client, CreateSessionRequest, Error, Event, EventKind, MAX_PAGE_SIZE,
    PageWalkLimits, RetryPolicy, ScreenSnapshot, Session, SessionId, SessionInput,
    SessionPageRequest, SessionStatus, TerminalGeometry, Usage,
};
use tokio::sync::Mutex;

const SESSION: &str = include_str!("fixtures/session.json");
const SESSIONS: &str = include_str!("fixtures/sessions.json");
const CREATE_REQUEST: &str = include_str!("fixtures/create_session_request.json");
const INPUT_TEXT_REQUEST: &str = include_str!("fixtures/input_text_request.json");
const INPUT_BYTES_REQUEST: &str = include_str!("fixtures/input_bytes_request.json");
const APPROVAL_REQUEST: &str = include_str!("fixtures/approval_request.json");
const RESIZE_REQUEST: &str = include_str!("fixtures/resize_request.json");
const EVENT: &str = include_str!("fixtures/event.json");
const CAPABILITY_REFUSED: &str = include_str!("fixtures/capability_refused.json");
const SCREEN: &str = include_str!("fixtures/screen.json");
const USAGE: &str = include_str!("fixtures/usage.json");

#[derive(Clone)]
struct StubState {
    responses: Arc<Mutex<VecDeque<StubResponse>>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

#[derive(Clone)]
struct StubResponse {
    status: StatusCode,
    content_type: Option<&'static str>,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
    delay: Duration,
}

impl StubResponse {
    fn json(status: StatusCode, body: &str) -> Self {
        Self {
            status,
            content_type: Some("application/json"),
            headers: Vec::new(),
            body: body.to_owned(),
            delay: Duration::ZERO,
        }
    }

    fn empty(status: StatusCode) -> Self {
        Self {
            status,
            content_type: None,
            headers: Vec::new(),
            body: String::new(),
            delay: Duration::ZERO,
        }
    }

    fn sse(frames: &[(u64, serde_json::Value)]) -> Self {
        let body = frames
            .iter()
            .map(|(id, data)| format!("id: {id}\ndata: {data}\n\n"))
            .collect();
        Self {
            status: StatusCode::OK,
            content_type: Some("text/event-stream; charset=utf-8"),
            headers: Vec::new(),
            body,
            delay: Duration::ZERO,
        }
    }

    fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }
}

#[derive(Clone)]
struct CapturedRequest {
    method: Method,
    uri: Uri,
    authorization: Option<String>,
    last_event_id: Option<String>,
    body: Vec<u8>,
}

struct Stub {
    base_url: String,
    state: StubState,
    server: tokio::task::JoinHandle<()>,
}

impl Stub {
    async fn start(responses: impl IntoIterator<Item = StubResponse>) -> Self {
        let state = StubState {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .fallback(any(capture))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("stub listener binds");
        let address = listener.local_addr().expect("stub address exists");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("stub server remains available");
        });
        Self {
            base_url: format!("http://{address}/proxy"),
            state,
            server,
        }
    }

    async fn requests(&self) -> Vec<CapturedRequest> {
        self.state.requests.lock().await.clone()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn capture(
    State(state): State<StubState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.requests.lock().await.push(CapturedRequest {
        method,
        uri,
        authorization: headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        last_event_id: headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        body: body.to_vec(),
    });
    let response = state
        .responses
        .lock()
        .await
        .pop_front()
        .unwrap_or_else(|| StubResponse::empty(StatusCode::INTERNAL_SERVER_ERROR));
    if !response.delay.is_zero() {
        tokio::time::sleep(response.delay).await;
    }
    let mut builder = Response::builder().status(response.status);
    if let Some(content_type) = response.content_type {
        builder = builder.header("content-type", content_type);
    }
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from(response.body))
        .expect("stub response builds")
}

fn no_delay_policy(max_attempts: usize) -> RetryPolicy {
    RetryPolicy::new(max_attempts, Duration::ZERO, Duration::ZERO)
        .expect("test retry policy is valid")
        .with_jitter(0.0)
        .expect("zero jitter is valid")
}

fn stub_client(stub: &Stub, attempts: usize) -> (Client, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&calls);
    let client = Client::builder(&stub.base_url)
        .expect("stub URL is valid")
        .credential_provider(move || {
            let call = provider_calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok::<_, Infallible>(format!("fixture-auth-{call}"))
        })
        .retry_policy(no_delay_policy(attempts))
        .build()
        .expect("client builds");
    (client, calls)
}

fn fixture(source: &str) -> serde_json::Value {
    serde_json::from_str(source).expect("checked-in fixture is JSON")
}

fn event_fixture(source: &str) -> serde_json::Value {
    fixture(source)
}

#[test]
fn sdk_copies_read_every_server_owned_response_fixture() {
    let session: Session = serde_json::from_str(SESSION).expect("session fixture is typed");
    assert_eq!(session.id.as_str(), "sesn_fixture");
    let sessions: Vec<Session> = serde_json::from_str(SESSIONS).expect("list fixture is typed");
    assert_eq!(sessions.len(), 1);
    let event: Event = serde_json::from_str(EVENT).expect("event fixture is typed");
    assert!(matches!(event.kind, EventKind::ApprovalRequested { .. }));
    let screen: ScreenSnapshot = serde_json::from_str(SCREEN).expect("screen fixture is typed");
    assert_eq!(screen.lines.len(), 3);
    let usage: Usage = serde_json::from_str(USAGE).expect("usage fixture is typed");
    assert_eq!(usage.context_window, Some(200_000));
}

#[tokio::test]
async fn every_required_operation_uses_the_server_fixture_shape() {
    let stub = Stub::start([
        StubResponse::json(StatusCode::CREATED, SESSION),
        StubResponse::json(StatusCode::OK, SESSIONS),
        StubResponse::json(StatusCode::OK, SESSION),
        StubResponse::empty(StatusCode::ACCEPTED),
        StubResponse::empty(StatusCode::ACCEPTED),
        StubResponse::empty(StatusCode::ACCEPTED),
        StubResponse::empty(StatusCode::ACCEPTED),
        StubResponse::empty(StatusCode::ACCEPTED),
        StubResponse::empty(StatusCode::NO_CONTENT),
        StubResponse::json(StatusCode::OK, SCREEN),
        StubResponse::json(StatusCode::OK, USAGE),
    ])
    .await;
    let (client, provider_calls) = stub_client(&stub, 1);
    let id = SessionId::new("sesn_fixture").expect("fixture id is valid");

    client
        .create_session(CreateSessionRequest {
            geometry: Some(TerminalGeometry::new(120, 40).expect("valid geometry")),
            environment_profile: Some("envp_fixture".into()),
            quoted_rate_card: Some("rate_fixture".into()),
        })
        .await
        .expect("create succeeds");
    client.list_sessions().await.expect("list succeeds");
    client.read_session(&id).await.expect("read succeeds");
    client
        .send_input(
            &id,
            SessionInput::text_without_newline("Only inspect src/auth."),
        )
        .await
        .expect("text input succeeds");
    client
        .send_input(&id, SessionInput::bytes([0x1b, 0x0b, 0x42]))
        .await
        .expect("byte input succeeds");
    client
        .resolve_approval(&id, "req_fixture", ApprovalDecision::Abort)
        .await
        .expect("approval succeeds");
    client.interrupt(&id).await.expect("interrupt succeeds");
    client
        .steer(&id, "Only inspect src/auth.")
        .await
        .expect("steer succeeds");
    client
        .resize_terminal(&id, TerminalGeometry::new(132, 44).expect("valid geometry"))
        .await
        .expect("resize succeeds");
    client.screen_snapshot(&id).await.expect("screen succeeds");
    client.get_usage(&id).await.expect("usage succeeds");

    let requests = stub.requests().await;
    assert_eq!(requests.len(), 11);
    let expected = [
        (Method::POST, "/proxy/api/v1/sessions"),
        (Method::GET, "/proxy/api/v1/sessions"),
        (Method::GET, "/proxy/api/v1/sessions/sesn_fixture"),
        (Method::POST, "/proxy/api/v1/sessions/sesn_fixture/input"),
        (Method::POST, "/proxy/api/v1/sessions/sesn_fixture/input"),
        (
            Method::POST,
            "/proxy/api/v1/sessions/sesn_fixture/approvals/req_fixture",
        ),
        (
            Method::POST,
            "/proxy/api/v1/sessions/sesn_fixture/interrupt",
        ),
        (Method::POST, "/proxy/api/v1/sessions/sesn_fixture/steer"),
        (Method::POST, "/proxy/api/v1/sessions/sesn_fixture/resize"),
        (Method::GET, "/proxy/api/v1/sessions/sesn_fixture/screen"),
        (Method::GET, "/proxy/api/v1/sessions/sesn_fixture/usage"),
    ];
    for (request, (method, path)) in requests.iter().zip(expected) {
        assert_eq!(request.method, method);
        assert_eq!(request.uri.path(), path);
    }
    for (index, request) in requests.iter().enumerate() {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer fixture-auth-{}", index + 1).as_str())
        );
    }
    assert_eq!(provider_calls.load(Ordering::SeqCst), 11);

    for (request, expected) in [
        (&requests[0], CREATE_REQUEST),
        (&requests[3], INPUT_TEXT_REQUEST),
        (&requests[4], INPUT_BYTES_REQUEST),
        (&requests[5], APPROVAL_REQUEST),
        (&requests[8], RESIZE_REQUEST),
    ] {
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&request.body)
                .expect("request body is JSON"),
            fixture(expected)
        );
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[7].body).expect("steer body is JSON"),
        serde_json::json!({ "text": "Only inspect src/auth." })
    );
}

#[tokio::test]
async fn list_sessions_finds_next_after_unrelated_link_header_fields() {
    let older = SESSIONS.replace("sesn_fixture", "sesn_older");
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, SESSIONS)
            .header(
                "link",
                "<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel=\"profile\"",
            )
            .header(
                "link",
                "</api/v1/sessions?limit=50&cursor=opaque>; rel=\"next\"",
            ),
        StubResponse::json(StatusCode::OK, &older),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);

    let sessions = client.list_sessions().await.expect("both pages list");

    assert_eq!(
        sessions
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        ["sesn_fixture", "sesn_older"]
    );
    assert_eq!(
        stub.requests()
            .await
            .iter()
            .map(|request| request.uri.to_string())
            .collect::<Vec<_>>(),
        [
            "/proxy/api/v1/sessions",
            "/proxy/api/v1/sessions?limit=50&cursor=opaque",
        ]
    );
}

#[tokio::test]
async fn list_sessions_accepts_an_unrelated_link_on_the_final_page() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, SESSIONS).header(
        "link",
        "<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel=\"profile\"",
    )])
    .await;
    let (client, _) = stub_client(&stub, 1);

    let sessions = client.list_sessions().await.expect("the final page lists");

    assert_eq!(sessions.len(), 1);
    assert_eq!(stub.requests().await.len(), 1);
}

#[tokio::test]
async fn list_session_page_exposes_filters_and_an_opaque_manual_continuation() {
    let older = SESSIONS.replace("sesn_fixture", "sesn_older");
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, SESSIONS).header(
            "link",
            "</api/v1/sessions?status=running&limit=1&cursor=opaque-next>; rel=\"next\"",
        ),
        StubResponse::json(StatusCode::OK, &older),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);
    let request = SessionPageRequest::new()
        .with_status(SessionStatus::Running)
        .expect("a first-page status filter is accepted")
        .with_limit(1)
        .expect("one is within the page bound")
        .with_cursor("opaque start")
        .expect("an opaque cursor is accepted");

    let first = client
        .list_session_page(request)
        .await
        .expect("first page lists");
    assert_eq!(first.items[0].id.as_str(), "sesn_fixture");
    let next = first.next.expect("first page continues");
    assert!(
        SessionPageRequest::continuing(next.clone())
            .with_status(SessionStatus::Ready)
            .is_err()
    );
    let last = client
        .list_session_page(SessionPageRequest::continuing(next))
        .await
        .expect("manual continuation lists");
    assert_eq!(last.items[0].id.as_str(), "sesn_older");
    assert!(last.next.is_none());

    assert_eq!(
        stub.requests()
            .await
            .iter()
            .map(|request| request.uri.to_string())
            .collect::<Vec<_>>(),
        [
            "/proxy/api/v1/sessions?status=running&limit=1&cursor=opaque+start",
            "/proxy/api/v1/sessions?status=running&limit=1&cursor=opaque-next",
        ]
    );
}

#[test]
fn bounded_page_and_walk_options_reject_invalid_edges() {
    assert!(SessionPageRequest::new().with_limit(0).is_err());
    assert!(
        SessionPageRequest::new()
            .with_limit(MAX_PAGE_SIZE + 1)
            .is_err()
    );
    assert!(SessionPageRequest::new().with_cursor("").is_err());
    assert!(PageWalkLimits::new(0, 1).is_err());
    assert!(PageWalkLimits::new(1, 0).is_err());
}

#[tokio::test]
async fn list_sessions_enforces_page_item_and_loop_boundaries() {
    let continuing = StubResponse::json(StatusCode::OK, SESSIONS)
        .header("link", "</api/v1/sessions?cursor=again>; rel=\"next\"");
    let page_stub = Stub::start([continuing.clone()]).await;
    let (page_client, _) = stub_client(&page_stub, 1);
    assert!(matches!(
        page_client
            .list_sessions_with_limits(PageWalkLimits::new(1, 10).expect("valid limits"))
            .await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(page_stub.requests().await.len(), 1);

    let two = format!("[{SESSION},{SESSION}]")
        .replace("\"id\": \"sesn_fixture\"", "\"id\": \"sesn_second\"");
    let item_stub = Stub::start([StubResponse::json(StatusCode::OK, &two)]).await;
    let (item_client, _) = stub_client(&item_stub, 1);
    assert!(matches!(
        item_client
            .list_sessions_with_limits(PageWalkLimits::new(2, 1).expect("valid limits"))
            .await,
        Err(Error::Protocol { .. })
    ));

    let loop_stub = Stub::start([continuing.clone(), continuing]).await;
    let (loop_client, _) = stub_client(&loop_stub, 1);
    assert!(matches!(
        loop_client.list_sessions().await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(loop_stub.requests().await.len(), 2);
}

#[tokio::test]
async fn list_sessions_refuses_a_link_that_could_redirect_the_bearer_credential() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, SESSIONS).header(
        "link",
        "<https://outside.example/api/v1/sessions?cursor=x>; rel=\"next\"",
    )])
    .await;
    let (client, _) = stub_client(&stub, 1);

    assert!(matches!(
        client.list_sessions().await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(stub.requests().await.len(), 1);
}

#[tokio::test]
async fn list_sessions_refuses_a_link_to_a_different_control_plane_route() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, SESSIONS).header(
        "link",
        "</api/v1/environment-profiles?cursor=x>; rel=\"next\"",
    )])
    .await;
    let (client, _) = stub_client(&stub, 1);

    assert!(matches!(
        client.list_sessions().await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(stub.requests().await.len(), 1);
}

#[tokio::test]
async fn list_sessions_refuses_a_malformed_entry_that_declares_next() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, SESSIONS)
        .header("link", "/api/v1/sessions?cursor=x; rel=\"next\"")])
    .await;
    let (client, _) = stub_client(&stub, 1);

    assert!(matches!(
        client.list_sessions().await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(stub.requests().await.len(), 1);
}

#[tokio::test]
async fn create_stream_interrupt_smoke_uses_a_fresh_credential_each_time() {
    let event = event_fixture(EVENT);
    let stub = Stub::start([
        StubResponse::json(StatusCode::CREATED, SESSION),
        StubResponse::sse(&[(8, event)]),
        StubResponse::empty(StatusCode::ACCEPTED),
    ])
    .await;
    let (client, provider_calls) = stub_client(&stub, 2);

    let session = client
        .create_session(CreateSessionRequest::default())
        .await
        .expect("session is created");
    let mut stream = client
        .stream_events(&session.id, Some(7))
        .await
        .expect("stream opens");
    let event = stream
        .next()
        .await
        .expect("one frame arrives")
        .expect("frame is valid");
    assert_eq!(event.seq, 8);
    drop(stream);
    client
        .interrupt(&session.id)
        .await
        .expect("session is interrupted");

    let requests = stub.requests().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1].uri.query(), Some("after=7"));
    assert_eq!(requests[1].last_event_id.as_deref(), Some("7"));
    assert_eq!(provider_calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn stream_reconnects_after_the_last_validated_cursor_and_deduplicates() {
    let first = event_fixture(EVENT);
    let duplicate = first.clone();
    let second = serde_json::json!({
        "seq": 9,
        "session_id": "sesn_fixture",
        "event": { "type": "turn_completed" }
    });
    let stub = Stub::start([
        StubResponse::sse(&[(8, first)]),
        StubResponse::sse(&[(8, duplicate), (9, second)]),
    ])
    .await;
    let (client, calls) = stub_client(&stub, 2);
    let id = SessionId::new("sesn_fixture").expect("valid id");
    let mut stream = client
        .stream_events(&id, Some(7))
        .await
        .expect("stream opens");
    assert_eq!(
        stream
            .next()
            .await
            .expect("first item")
            .expect("first event")
            .seq,
        8
    );
    let next = stream
        .next()
        .await
        .expect("reconnected item")
        .expect("reconnected event");
    assert_eq!(next.seq, 9);
    assert!(matches!(next.kind, EventKind::TurnCompleted));

    let requests = stub.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].uri.query(), Some("after=8"));
    assert_eq!(requests[1].last_event_id.as_deref(), Some("8"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn capability_refusal_is_publicly_distinct_from_transport_failure() {
    let stub = Stub::start([StubResponse::json(
        StatusCode::UNPROCESSABLE_ENTITY,
        CAPABILITY_REFUSED,
    )])
    .await;
    let (client, _) = stub_client(&stub, 1);
    let id = SessionId::new("sesn_fixture").expect("valid id");
    let error = client
        .steer(&id, "continue")
        .await
        .expect_err("steer is refused");
    assert!(matches!(
        error,
        Error::CapabilityRefused { ref capability } if capability == "steer"
    ));

    let unavailable = Client::builder("https://example.test")
        .expect("valid URL")
        .credential_provider(|| Err::<String, _>("provider is unavailable"))
        .retry_policy(no_delay_policy(1))
        .build()
        .expect("client builds");
    assert!(matches!(
        unavailable.list_sessions().await,
        Err(Error::Transport { .. })
    ));
}

#[tokio::test]
async fn retry_after_delta_and_http_date_are_honored_before_retrying() {
    for retry_after in ["0", "Sun, 06 Nov 1994 08:49:37 GMT"] {
        let unavailable = serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/runtime-unavailable",
            "title": "runtime unavailable",
            "status": 503,
            "detail": "runtime is unavailable",
            "code": "runtime_unavailable"
        })
        .to_string();
        let stub = Stub::start([
            StubResponse::json(StatusCode::SERVICE_UNAVAILABLE, &unavailable)
                .header("retry-after", retry_after),
            StubResponse::json(StatusCode::OK, SESSIONS),
        ])
        .await;
        let (client, calls) = stub_client(&stub, 2);
        client.list_sessions().await.expect("retry succeeds");
        assert_eq!(stub.requests().await.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test]
async fn an_api_failure_without_retry_after_is_not_retried() {
    let unavailable = serde_json::json!({
        "type": "https://spec.chrono-ai.fun/cma/1.0/problems/runtime-unavailable",
        "title": "runtime unavailable",
        "status": 503,
        "detail": "runtime is unavailable",
        "code": "runtime_unavailable"
    })
    .to_string();
    let stub = Stub::start([
        StubResponse::json(StatusCode::SERVICE_UNAVAILABLE, &unavailable),
        StubResponse::json(StatusCode::OK, SESSIONS),
    ])
    .await;
    let (client, calls) = stub_client(&stub, 2);

    let error = client
        .list_sessions()
        .await
        .expect_err("an API refusal is returned immediately");
    assert!(matches!(
        error,
        Error::Api {
            status: 503,
            ref code,
            ..
        } if code == "runtime_unavailable"
    ));
    assert_eq!(stub.requests().await.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn transport_timeout_retries_and_refreshes_the_credential() {
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, SESSIONS).delayed(Duration::from_millis(100)),
        StubResponse::json(StatusCode::OK, SESSIONS),
    ])
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&calls);
    let client = Client::builder(&stub.base_url)
        .expect("valid URL")
        .credential_provider(move || {
            let call = provider_calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok::<_, Infallible>(format!("fixture-retry-{call}"))
        })
        .request_timeout(Duration::from_millis(20))
        .expect("timeout is valid")
        .retry_policy(no_delay_policy(2))
        .build()
        .expect("client builds");

    client
        .list_sessions()
        .await
        .expect("second attempt succeeds");
    assert_eq!(stub.requests().await.len(), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn dropping_a_pending_call_does_not_poison_the_client() {
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, SESSIONS).delayed(Duration::from_secs(1)),
        StubResponse::json(StatusCode::OK, SESSIONS),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);
    let pending_client = client.clone();
    let pending = tokio::spawn(async move { pending_client.list_sessions().await });

    for _ in 0..50 {
        if !stub.requests().await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    pending.abort();
    client
        .list_sessions()
        .await
        .expect("client remains usable after cancellation");
    assert_eq!(stub.requests().await.len(), 2);
}

#[tokio::test]
async fn malformed_success_and_event_boundaries_are_protocol_errors() {
    let malformed_session = Stub::start([StubResponse::json(StatusCode::OK, "{\"id\":1}")]).await;
    let (client, _) = stub_client(&malformed_session, 1);
    assert!(matches!(
        client.list_sessions().await,
        Err(Error::Protocol { .. })
    ));

    let mismatched = serde_json::json!({
        "seq": 9,
        "session_id": "sesn_fixture",
        "event": { "type": "turn_started" }
    });
    let stub = Stub::start([StubResponse::sse(&[(8, mismatched)])]).await;
    let (client, _) = stub_client(&stub, 1);
    let id = SessionId::new("sesn_fixture").expect("valid id");
    let mut stream = client
        .stream_events(&id, None)
        .await
        .expect("handshake succeeds");
    assert!(matches!(
        stream.next().await,
        Some(Err(Error::Protocol { .. }))
    ));
}

#[tokio::test]
async fn unknown_event_variants_cross_the_live_stream_intact() {
    let payload = serde_json::json!({
        "seq": 10,
        "session_id": "sesn_fixture",
        "event": { "type": "future_event", "nested": { "answer": 42 } }
    });
    let stub = Stub::start([StubResponse::sse(&[(10, payload)])]).await;
    let (client, _) = stub_client(&stub, 1);
    let id = SessionId::new("sesn_fixture").expect("valid id");
    let mut stream = client.stream_events(&id, None).await.expect("stream opens");
    let event = stream.next().await.expect("item").expect("valid event");
    let EventKind::Unknown {
        event_type,
        payload,
    } = event.kind
    else {
        panic!("future event must use the unknown carrier");
    };
    assert_eq!(event_type, "future_event");
    assert_eq!(payload["nested"]["answer"], 42);
}
