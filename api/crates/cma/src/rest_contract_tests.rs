//! Production-composition checks for the REST error contract.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode},
    Extension, Router,
};
use layer1_public::{tickets::TerminalTickets, AuthenticatedSubject};
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, AgentConfiguration, Authorization, Baseline,
    Caller, Capability, EnvironmentProfile, EnvironmentProfileId, EnvironmentProfileService,
    EnvironmentProfileStore, Event, KernelId, PlatformSettingsService, RegisteredUser, Reserved,
    ScreenFactory, ScreenModel, ScreenSnapshot, SessionId, SessionPorts, SessionRecord,
    SessionService, SessionStatus, SessionStore, StoreError, StoredEvent, SubjectId,
    TerminalGeometry, UserDirectory, UserDirectoryStore,
};
use layer3_adapter::{
    AdapterError, KernelAdapter, KernelCapabilities, RuntimeHandle, RuntimeSpec, TerminalChannel,
};
use tokio::sync::{mpsc, Mutex};
use tower::ServiceExt;

use super::{rest_api_router, with_cors};
use crate::activity::{self, ActivityRecorder};

const OVERSIZED_BODY_LEN: usize = 2 * 1024 * 1024 + 1;
const LISTED_ORIGIN: &str = "https://integrator.example";
const UNLISTED_ORIGIN: &str = "https://unlisted.example";

#[derive(Default)]
struct MemorySessions {
    records: Mutex<HashMap<String, SessionRecord>>,
    events: Mutex<Vec<Event>>,
    fail_lists: AtomicBool,
}

#[async_trait::async_trait]
impl SessionStore for MemorySessions {
    async fn upsert(&self, record: &SessionRecord) -> Result<(), StoreError> {
        self.records
            .lock()
            .await
            .insert(record.session.id.0.clone(), record.clone());
        Ok(())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self.records.lock().await.get(&id.0).cloned())
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<SessionRecord>, StoreError> {
        if self.fail_lists.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        Ok(self
            .records
            .lock()
            .await
            .values()
            .filter(|record| &record.session.owner == owner)
            .cloned()
            .collect())
    }

    async fn reserved_capacity(&self) -> Result<Reserved, StoreError> {
        let mut reserved = Reserved::default();
        for record in self.records.lock().await.values() {
            if record.session.status.wants_runtime() {
                reserved.include(record.session.options.size);
            }
        }
        Ok(reserved)
    }

    async fn list_all(&self) -> Result<Vec<SessionRecord>, StoreError> {
        if self.fail_lists.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        Ok(self.records.lock().await.values().cloned().collect())
    }

    async fn delete(&self, id: &SessionId) -> Result<(), StoreError> {
        self.records.lock().await.remove(&id.0);
        self.events
            .lock()
            .await
            .retain(|event| &event.session_id != id);
        Ok(())
    }

    async fn append_events(&self, events: &[Event]) -> Result<(), StoreError> {
        self.events.lock().await.extend_from_slice(events);
        Ok(())
    }

    async fn events_since(
        &self,
        id: &SessionId,
        after: u64,
    ) -> Result<Vec<StoredEvent>, StoreError> {
        let mut events: Vec<_> = self
            .events
            .lock()
            .await
            .iter()
            .filter(|event| &event.session_id == id && event.seq > after)
            .cloned()
            .map(StoredEvent::from)
            .collect();
        events.sort_by_key(|event| event.seq);
        Ok(events)
    }
}

struct NoProfiles;

#[async_trait::async_trait]
impl EnvironmentProfileStore for NoProfiles {
    async fn upsert(&self, _profile: &EnvironmentProfile) -> Result<(), StoreError> {
        Ok(())
    }

    async fn get(
        &self,
        _id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        Ok(None)
    }

    async fn list_for(&self, _owner: &SubjectId) -> Result<Vec<EnvironmentProfile>, StoreError> {
        Ok(Vec::new())
    }

    async fn delete(&self, _id: &EnvironmentProfileId) -> Result<(), StoreError> {
        Ok(())
    }
}

struct NoDirectory;

#[async_trait::async_trait]
impl UserDirectoryStore for NoDirectory {
    async fn record_sign_in(&self, _user: &RegisteredUser) -> Result<(), StoreError> {
        Ok(())
    }

    async fn recent(
        &self,
        _query: &layer2_core::DirectoryQuery,
    ) -> Result<Vec<RegisteredUser>, StoreError> {
        Ok(Vec::new())
    }

    async fn by_subjects(
        &self,
        _subjects: &[SubjectId],
    ) -> Result<Vec<RegisteredUser>, StoreError> {
        Ok(Vec::new())
    }
}

struct NoActivity;

#[async_trait::async_trait]
impl layer2_core::ActivityStore for NoActivity {
    async fn append(&self, _record: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn recent(&self, _query: &ActivityQuery) -> Result<Vec<ActivityRecord>, StoreError> {
        Ok(Vec::new())
    }
}

struct EmptyScreen(TerminalGeometry);

impl ScreenModel for EmptyScreen {
    fn apply(&mut self, _bytes: &[u8]) {}

    fn resize(&mut self, geometry: TerminalGeometry) {
        self.0 = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.0,
            rows: Vec::new(),
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        Vec::new()
    }
}

struct TestKernel;

#[async_trait::async_trait]
impl KernelAdapter for TestKernel {
    fn kernel_name(&self) -> &'static str {
        "rest-contract-test"
    }

    fn capabilities(&self) -> KernelCapabilities {
        KernelCapabilities {
            terminal: true,
            ..Default::default()
        }
    }

    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        Ok(RuntimeHandle(format!("runtime-{}", spec.session_id)))
    }

    async fn attach_terminal(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        let (input, mut received) = mpsc::channel(8);
        let (output, produced) = mpsc::channel(8);
        tokio::spawn(async move {
            let _keep_output_open = output;
            while received.recv().await.is_some() {}
        });
        Ok(TerminalChannel {
            output: produced,
            input,
        })
    }

    async fn refresh_credential(
        &self,
        _handle: &RuntimeHandle,
        _credential: &str,
    ) -> Result<(), AdapterError> {
        Ok(())
    }

    async fn terminate(&self, _handle: &RuntimeHandle) -> Result<(), AdapterError> {
        Ok(())
    }
}

struct TestSurface {
    app: Router,
    sessions: Arc<SessionService>,
    store: Arc<MemorySessions>,
    caller: Caller,
}

fn test_surface(authenticated: bool) -> TestSurface {
    let settings = Arc::new(PlatformSettingsService::unconfigured());
    let environments = EnvironmentProfileService::new(
        Arc::new(NoProfiles),
        Baseline::default(),
        None,
        None,
        AgentConfiguration {
            kernel: KernelId::new("test"),
            ..Default::default()
        },
        Arc::clone(&settings),
    );
    let mut adapters: HashMap<KernelId, Arc<dyn KernelAdapter>> = HashMap::new();
    adapters.insert(KernelId::new("test"), Arc::new(TestKernel));
    let factory: ScreenFactory = Arc::new(|geometry| Box::new(EmptyScreen(geometry)));
    let store = Arc::new(MemorySessions::default());
    let sessions = SessionService::new(
        adapters,
        factory,
        Arc::clone(&store) as Arc<dyn SessionStore>,
        SessionPorts {
            settings: Some(Arc::clone(&settings)),
            ..Default::default()
        },
        Arc::clone(&environments),
        Default::default(),
    );
    let users = UserDirectory::new(Arc::new(NoDirectory));
    let activity = ActivityService::new(Arc::new(NoActivity));
    let caller = Caller::new(
        SubjectId("subject-contract".into()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ManageOwnEnvironmentProfiles,
            Capability::ReadUserDirectory,
            Capability::ReadActivityLog,
            Capability::ConfigurePlatform,
        ]),
    );
    let app = rest_api_router(
        layer1_public::rest::SessionsState {
            sessions: Arc::clone(&sessions),
            users: Arc::clone(&users),
            tickets: TerminalTickets::new(),
            environments: Arc::clone(&environments),
        },
        Arc::new(layer1_public::rest::admin::AdminState {
            sessions: Arc::clone(&sessions),
            activity: Arc::clone(&activity),
            users,
            settings,
        }),
    )
    .layer(axum::middleware::from_fn_with_state(
        ActivityRecorder::new(activity),
        activity::record,
    ));
    let app = if authenticated {
        app.layer(Extension(caller.authorization.clone()))
            .layer(Extension(AuthenticatedSubject(caller.subject.clone())))
    } else {
        app
    };
    TestSurface {
        app,
        sessions,
        store,
        caller,
    }
}

fn json_request(method: Method, uri: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body.into())
        .expect("request builds")
}

fn preflight_request(
    uri: &str,
    origin: &'static str,
    requested_method: Method,
    requested_headers: Option<&'static str>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(Method::OPTIONS)
        .uri(uri)
        .header(header::ORIGIN, origin)
        .header(
            header::ACCESS_CONTROL_REQUEST_METHOD,
            requested_method.as_str(),
        );
    if let Some(requested_headers) = requested_headers {
        request = request.header(header::ACCESS_CONTROL_REQUEST_HEADERS, requested_headers);
    }
    request
        .body(Body::empty())
        .expect("preflight request builds")
}

fn assert_header_list_contains(headers: &HeaderMap, name: HeaderName, expected: &str) {
    let value = headers
        .get(&name)
        .unwrap_or_else(|| panic!("response is missing {name}"))
        .to_str()
        .unwrap_or_else(|_| panic!("{name} is not text"));
    assert!(
        value
            .split(',')
            .any(|item| item.trim().eq_ignore_ascii_case(expected)),
        "{name} did not contain {expected:?}: {value:?}"
    );
}

async fn assert_problem(response: Response<Body>, status: StatusCode) -> serde_json::Value {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static(
            "application/problem+json"
        ))
    );
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("problem documents are small"),
    )
    .expect("the problem document is json");
    assert_eq!(body["status"], status.as_u16());
    assert!(
        body["code"].as_str().is_some_and(|code| !code.is_empty()),
        "problem code must be non-empty: {body}"
    );
    body
}

async fn assert_request_too_large(response: Response<Body>) {
    let body = assert_problem(response, StatusCode::PAYLOAD_TOO_LARGE).await;
    assert_eq!(body["code"], "request_too_large");
    assert_eq!(
        body["detail"],
        "The request body exceeds what this service accepts."
    );
}

async fn create_ready_session(
    app: &Router,
    sessions: &SessionService,
    caller: &Caller,
) -> SessionId {
    let response = app
        .clone()
        .oneshot(json_request(Method::POST, "/api/v1/sessions", "{}"))
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("session responses are bounded"),
    )
    .expect("the session response is json");
    let id = SessionId(body["id"].as_str().expect("session id").to_string());

    for _ in 0..200 {
        let session = sessions
            .get_session(caller, &id)
            .await
            .expect("the created session is readable");
        if session.status == SessionStatus::Ready {
            return id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the in-memory runtime did not become ready");
}

#[tokio::test]
async fn listed_origin_completes_an_authorized_http_cors_sequence() {
    let app = with_cors(
        test_surface(true).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("loopback listener binds");
    let address = listener.local_addr().expect("listener has an address");
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let client = reqwest::Client::new();
    let response = client
        .request(Method::OPTIONS, format!("http://{address}/api/v1/sessions"))
        .header(header::ORIGIN, LISTED_ORIGIN)
        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            header::ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,content-type",
        )
        .send()
        .await
        .expect("the HTTP server answers the preflight");
    let status = response.status();
    let headers = response.headers().clone();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .expect("listed origin is allowed"),
        LISTED_ORIGIN
    );
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .expect("credential mode is retained"),
        "true"
    );
    assert_header_list_contains(
        &headers,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "authorization",
    );
    assert_header_list_contains(
        &headers,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "content-type",
    );
    assert_header_list_contains(&headers, header::ACCESS_CONTROL_ALLOW_METHODS, "POST");
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_MAX_AGE)
            .expect("preflight decisions are cacheable"),
        "3600"
    );

    let response = client
        .post(format!("http://{address}/api/v1/sessions"))
        .header(header::ORIGIN, LISTED_ORIGIN)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer browser-fixture")
        .body("{}")
        .send()
        .await
        .expect("the admitted browser request reaches the HTTP server");
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .expect("the actual response retains the browser grant"),
        LISTED_ORIGIN
    );
    let body = response
        .json::<serde_json::Value>()
        .await
        .expect("the authenticated create response is JSON");
    assert!(
        body["id"].is_string(),
        "the authenticated call created no session: {body}"
    );

    shutdown_tx.send(()).expect("the server is still running");
    server
        .await
        .expect("the server task joins")
        .expect("the server stops cleanly");
}

#[tokio::test]
async fn listed_origin_allows_the_sdk_reconnect_cursor_header() {
    let app = with_cors(
        test_surface(true).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    let response = app
        .oneshot(preflight_request(
            "/api/v1/sessions/session/events?after=17",
            LISTED_ORIGIN,
            Method::GET,
            Some("authorization,last-event-id"),
        ))
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(LISTED_ORIGIN))
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "authorization",
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        "last-event-id",
    );
}

#[tokio::test]
async fn listed_origin_allows_profile_put_preflight() {
    let app = with_cors(
        test_surface(true).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    let response = app
        .oneshot(preflight_request(
            "/api/v1/environment-profiles/x",
            LISTED_ORIGIN,
            Method::PUT,
            None,
        ))
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static(LISTED_ORIGIN))
    );
    assert_header_list_contains(
        response.headers(),
        header::ACCESS_CONTROL_ALLOW_METHODS,
        "PUT",
    );
}

#[tokio::test]
async fn unlisted_origin_receives_no_cors_grant() {
    let app = with_cors(
        test_surface(true).app,
        vec![HeaderValue::from_static(LISTED_ORIGIN)],
    );
    let response = app
        .oneshot(preflight_request(
            "/api/v1/sessions",
            UNLISTED_ORIGIN,
            Method::POST,
            Some("authorization"),
        ))
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "an unlisted origin must not receive a browser grant"
    );
}

#[tokio::test]
async fn every_json_route_rejects_malformed_bodies_as_problem_documents() {
    let app = test_surface(true).app;
    let routes = [
        (Method::POST, "/api/v1/sessions"),
        (Method::POST, "/api/v1/sessions/sesn_bad/input"),
        (Method::POST, "/api/v1/sessions/sesn_bad/resize"),
        (
            Method::POST,
            "/api/v1/sessions/sesn_bad/approvals/request_bad",
        ),
        (Method::POST, "/api/v1/sessions/sesn_bad/steer"),
        (Method::POST, "/api/v1/sessions/sesn_bad/plane"),
        (Method::POST, "/api/v1/sessions/sesn_bad/terminal-ticket"),
        (Method::POST, "/api/v1/environment-profiles"),
        (Method::PUT, "/api/v1/environment-profiles/preferred"),
        (Method::PUT, "/api/v1/environment-profiles/envp_bad"),
        (Method::PATCH, "/api/v1/admin/settings"),
        (Method::PUT, "/api/v1/admin/settings"),
        (Method::POST, "/api/v1/admin/settings/reset"),
    ];

    for (method, uri) in routes {
        let response = app
            .clone()
            .oneshot(json_request(method.clone(), uri, "{"))
            .await
            .expect("router answers");
        let body = assert_problem(response, StatusCode::BAD_REQUEST).await;
        assert_eq!(body["code"], "malformed_json", "for {method} {uri}");
    }
}

#[tokio::test]
async fn bad_query_strings_are_problem_documents_on_every_query_route() {
    let app = test_surface(true).app;
    for uri in [
        "/api/v1/sessions/sesn_bad/events?after=not-a-number",
        "/api/v1/pricing/quote?environment_profile=one&environment_profile=two",
        "/api/v1/admin/users?limit=not-a-number",
        "/api/v1/admin/activity?limit=not-a-number",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get(uri)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        let body = assert_problem(response, StatusCode::BAD_REQUEST).await;
        assert_eq!(body["code"], "invalid_query", "for {uri}");
    }
}

#[tokio::test]
async fn a_wrong_json_media_type_is_itself_a_problem_document() {
    let response = test_surface(true)
        .app
        .oneshot(
            Request::post("/api/v1/sessions")
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("{}"))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNSUPPORTED_MEDIA_TYPE).await;
    assert_eq!(body["code"], "unsupported_media_type");
}

#[tokio::test]
async fn a_schema_error_never_reflects_the_submitted_value() {
    let planted = "not-a-real-secret-value";
    let body = format!(r#"{{"name":"profile","variables":"{planted}"}}"#);
    let response = test_surface(true)
        .app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/environment-profiles",
            body,
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "invalid_request");
    let rendered = serde_json::to_string(&body).expect("problem serializes");
    assert!(
        !rendered.contains(planted),
        "problem reflected input: {body}"
    );
    assert_eq!(
        body["detail"],
        "The JSON body does not match this operation's request schema."
    );
}

#[tokio::test]
async fn the_optional_ticket_body_distinguishes_empty_from_untyped() {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_session(&app, &sessions, &caller).await;
    let uri = format!("/api/v1/sessions/{}/terminal-ticket", id.0);

    let response = app
        .clone()
        .oneshot(
            Request::post(&uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let ticket: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("ticket responses are bounded"),
    )
    .expect("the ticket is json");
    assert!(ticket["ticket"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));

    let response = app
        .clone()
        .oneshot(
            Request::post(&uri)
                .body(Body::from(r#"{"takeover":true}"#))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNSUPPORTED_MEDIA_TYPE).await;
    assert_eq!(body["code"], "unsupported_media_type");

    let planted = "not-a-real-ticket-secret";
    let response = app
        .oneshot(json_request(
            Method::POST,
            &uri,
            format!(r#"{{"takeover":"{planted}"}}"#),
        ))
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNPROCESSABLE_ENTITY).await;
    assert_eq!(body["code"], "invalid_request");
    assert!(!serde_json::to_string(&body)
        .expect("problem serializes")
        .contains(planted));
}

#[tokio::test]
async fn an_authenticated_route_without_a_credential_returns_the_coded_401() {
    let response = test_surface(false)
        .app
        .oneshot(
            Request::get("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::UNAUTHORIZED).await;
    assert_eq!(body["code"], "not_authenticated");
}

#[tokio::test]
async fn session_list_distinguishes_empty_success_from_a_store_outage() {
    let TestSurface {
        app,
        sessions,
        store,
        caller,
    } = test_surface(true);

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("session lists are bounded"),
    )
    .expect("the session list is json");
    assert_eq!(body, serde_json::json!([]));

    let id = create_ready_session(&app, &sessions, &caller).await;
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("session lists are bounded"),
    )
    .expect("the session list is json");
    assert_eq!(body.as_array().map(Vec::len), Some(1));
    assert_eq!(body[0]["id"], id.0);

    store.fail_lists.store(true, Ordering::SeqCst);
    let response = app
        .oneshot(
            Request::get("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::SERVICE_UNAVAILABLE).await;
    assert_eq!(body["code"], "runtime_unavailable");
    assert!(body["type"]
        .as_str()
        .is_some_and(|problem_type| problem_type.ends_with("/runtime-unavailable")));
}

#[tokio::test]
async fn a_domain_refusal_uses_the_same_problem_contract() {
    let response = test_surface(true)
        .app
        .oneshot(
            Request::get("/api/v1/sessions/sesn_missing")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let body = assert_problem(response, StatusCode::NOT_FOUND).await;
    assert_eq!(body["code"], "session_not_found");
}

#[tokio::test]
async fn the_production_body_capture_limit_returns_a_coded_413() {
    let response = test_surface(true)
        .app
        .oneshot(json_request(
            Method::POST,
            "/api/v1/sessions",
            vec![b' '; OVERSIZED_BODY_LEN],
        ))
        .await
        .expect("router answers");
    assert_request_too_large(response).await;
}

#[tokio::test]
async fn unbuffered_json_and_optional_body_limits_return_the_same_coded_413() {
    let app = test_surface(true).app;
    for (uri, content_type) in [
        ("/api/v1/sessions/sesn_bad/input", Some("application/json")),
        (
            "/api/v1/sessions/sesn_bad/terminal-ticket",
            Some("application/json"),
        ),
        ("/api/v1/sessions/sesn_bad/terminal-ticket", None),
    ] {
        let mut request = Request::post(uri);
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        let response = app
            .clone()
            .oneshot(
                request
                    .body(Body::from(vec![b' '; OVERSIZED_BODY_LEN]))
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        assert_request_too_large(response).await;
    }
}

#[tokio::test]
async fn rest_routing_failures_are_scoped_problem_documents() {
    let app = test_surface(true).app;

    for uri in ["/api/v1", "/api/v1/", "/api/v1/no-such-rest-route"] {
        let response = app
            .clone()
            .oneshot(
                Request::get(uri)
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router answers");
        let body = assert_problem(response, StatusCode::NOT_FOUND).await;
        assert_eq!(body["code"], "not_found", "for {uri}");
    }

    let response = app
        .clone()
        .oneshot(
            Request::put("/api/v1/sessions")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    let allow = response
        .headers()
        .get(header::ALLOW)
        .and_then(|value| value.to_str().ok())
        .expect("method refusals retain the Allow header");
    assert!(allow.split(',').any(|method| method == "GET"));
    assert!(allow.split(',').any(|method| method == "POST"));
    let body = assert_problem(response, StatusCode::METHOD_NOT_ALLOWED).await;
    assert_eq!(body["code"], "method_not_allowed");

    let response = app
        .oneshot(
            Request::get("/auth/not-a-browser-route")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_ne!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&header::HeaderValue::from_static(
            "application/problem+json"
        )),
        "the REST fallback must remain scoped to /api/v1"
    );
}

#[tokio::test]
async fn well_formed_create_and_input_keep_their_success_statuses() {
    let TestSurface {
        app,
        sessions,
        caller,
        ..
    } = test_surface(true);
    let id = create_ready_session(&app, &sessions, &caller).await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            &format!("/api/v1/sessions/{}/input", id.0),
            r#"{"text":"echo ready"}"#,
        ))
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}
