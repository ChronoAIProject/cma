use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::Response;
use axum::routing::any;
use cma_sdk::{
    ApprovalDecision, BeginConnectionRequest, ChatAgentSettings, ChatHostId, ChatHostPageRequest,
    ChatHostSettings, ChatHostStatus, ChatInactivity, ChatSettings, ChatSetup, ChatWorkspaceSource,
    Client, Conformance, CreateSessionRequest, CreateTriggerRequest, DiffReview, Error, Event,
    EventKind, Launch, LaunchConfirmationAction, LaunchConfirmationRequest, LaunchEventKind,
    LaunchListRequest, LaunchProgress, LaunchStartRequest, LaunchState, MAX_PAGE_SIZE,
    PageWalkLimits, PublishTriggerRequest, PullRequestConfirmRequest, PullRequestDraft,
    PullRequestOperation, PullRequestReviewRequest, PullRequestStatusRefreshRequest,
    ReasoningEffort, RecoverInitialPromptRequest, ResponseMetadata, RetryPolicy, ScreenSnapshot,
    SelectLaunchConnectionRequest, Session, SessionId, SessionInput, SessionPageRequest,
    SessionSize, SessionStatus, SessionSurface, SessionSurfaceFilter, TerminalGeometry,
    TriggerConnection, TriggerConnectionState, TriggerContentDraft, TriggerDraft, TriggerPreview,
    TriggerPreviewAvailability, TriggerPreviewCost, TriggerPreviewUnavailableReason,
    TriggerRepositorySelection, TriggerRevision, UpdateLaunchOverridesRequest,
    UpdateTriggerRequest, Usage,
};
use tokio::sync::Mutex;

const SESSION: &str = include_str!("fixtures/session.json");
const SESSIONS: &str = include_str!("fixtures/sessions.json");
const CHAT_HOSTS: &str = include_str!("fixtures/chat_hosts.json");
const CREATE_REQUEST: &str = include_str!("fixtures/create_session_request.json");
const INPUT_TEXT_REQUEST: &str = include_str!("fixtures/input_text_request.json");
const INPUT_BYTES_REQUEST: &str = include_str!("fixtures/input_bytes_request.json");
const APPROVAL_REQUEST: &str = include_str!("fixtures/approval_request.json");
const RESIZE_REQUEST: &str = include_str!("fixtures/resize_request.json");
const EVENT: &str = include_str!("fixtures/event.json");
const CAPABILITY_REFUSED: &str = include_str!("fixtures/capability_refused.json");
const SCREEN: &str = include_str!("fixtures/screen.json");
const USAGE: &str = include_str!("fixtures/usage.json");
const TRIGGER_PREVIEW: &str = include_str!("fixtures/trigger_preview.json");
const REPOSITORY_SELECTION: &str = include_str!("fixtures/repository_selection.json");
const LAUNCH: &str = include_str!("fixtures/launch.json");
const LAUNCH_PROGRESS: &str = include_str!("fixtures/launch_progress.json");
const DIFF_REVIEW: &str = include_str!("fixtures/diff_review.json");
const PULL_REQUEST_DRAFT: &str = include_str!("fixtures/pull_request_draft.json");
const LAUNCH_START_REQUEST: &str = include_str!("fixtures/launch_start_request.json");
const PULL_REQUEST_REVIEW_REQUEST: &str = include_str!("fixtures/pull_request_review_request.json");
const PULL_REQUEST_CONFIRM_REQUEST: &str =
    include_str!("fixtures/pull_request_confirm_request.json");
const PULL_REQUEST_STATUS_REFRESH_REQUEST: &str =
    include_str!("fixtures/pull_request_status_refresh_request.json");
const CONFORMANCE: &str = r#"{
  "conformsTo": ["https://spec.chrono-ai.fun/cma/1.0/conf/rest-core"],
  "version": "development",
  "build_sha": "development"
}"#;

#[derive(Clone)]
struct StubState {
    responses: Arc<Mutex<VecDeque<StubResponse>>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

#[derive(Clone)]
struct StubResponse {
    status: StatusCode,
    content_type: Option<&'static str>,
    headers: Vec<(HeaderName, HeaderValue)>,
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
        self.headers.push((
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        ));
        self
    }

    fn raw_header(mut self, name: &'static str, value: &[u8]) -> Self {
        self.headers.push((
            HeaderName::from_static(name),
            HeaderValue::from_bytes(value).expect("stub header value is HTTP-compatible"),
        ));
        self
    }
}

#[derive(Clone)]
struct CapturedRequest {
    method: Method,
    uri: Uri,
    authorization: Option<String>,
    request_id: Option<String>,
    idempotency_key: Option<String>,
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
        request_id: headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
        idempotency_key: headers
            .get("idempotency-key")
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

fn trigger_content_draft() -> TriggerContentDraft {
    TriggerContentDraft {
        schema_version: 1,
        publisher_display_identity: "Chrono Maintainer".into(),
        repository: cma_sdk::RepositorySelectionDraft {
            repository_id: 10,
            selected_ref: "main".into(),
            contribution_base_ref: "main".into(),
            source_mode: cma_sdk::TriggerSourceMode::PinnedCommit,
        },
        task: Default::default(),
        session: Default::default(),
        repository_access: Default::default(),
        setup: Default::default(),
        contribution: Default::default(),
        commercial_policy: Default::default(),
        override_policy: Default::default(),
        public_projection: Default::default(),
    }
}

fn trigger_draft_response() -> String {
    let launch = fixture(LAUNCH);
    serde_json::json!({
        "id": "trigger_fixture",
        "selected_service_id": "service_fixture",
        "version": 1,
        "next_revision_number": 2,
        "content": launch["revision_content"],
        "created_at": "2026-08-26T12:00:00Z",
        "updated_at": "2026-08-26T12:00:00Z"
    })
    .to_string()
}

fn trigger_revision_response() -> String {
    let launch = fixture(LAUNCH);
    serde_json::json!({
        "id": "revision_fixture",
        "public_id": "public_fixture",
        "digest": launch["revision_digest"],
        "availability": "published",
        "content": launch["revision_content"],
        "authority": {
            "service_id": "service_fixture",
            "service_state_version": 2,
            "account_id": 20,
            "account_login": "upstream-owner",
            "role": "owner",
            "verified_at": "2026-08-26T12:00:00Z"
        },
        "published_at": "2026-08-26T12:00:00Z",
        "retired_at": null
    })
    .to_string()
}

fn trigger_connection_response() -> String {
    serde_json::json!({
        "service_id": "service_fixture",
        "state_version": 2,
        "state": "active",
        "account_id": 21,
        "account_node_id": "U_contributor",
        "account_login": "contributor",
        "avatar_url": null,
        "granted_scopes": ["public_repo"],
        "consent_observed_at": "2026-08-26T12:00:00Z"
    })
    .to_string()
}

fn revoked_trigger_connection_response() -> String {
    let mut connection = fixture(&trigger_connection_response());
    connection["state_version"] = serde_json::json!(3);
    connection["state"] = serde_json::json!("inactive");
    connection.to_string()
}

fn connection_authorization_response() -> String {
    serde_json::json!({
        "service_id": "service_fixture",
        "authorization_url": "https://identity.example.test/authorize",
        "attempt_nonce": "attempt_fixture"
    })
    .to_string()
}

fn chat_settings() -> ChatSettings {
    ChatSettings {
        enabled: true,
        workspace: ChatWorkspaceSource {
            repository_url: "https://example.test/team/repository.git".into(),
            branch: "development".into(),
        },
        agent: ChatAgentSettings {
            model: Some("reasoning-model".into()),
            reasoning_effort: ReasoningEffort::XHigh,
            instructions: "Read the repository guidance.".into(),
            record_tool_bodies: false,
        },
        message_prefixes: vec!["/formalize".into(), "/check".into()],
        inactivity: ChatInactivity {
            pause_after_seconds: 1_200,
        },
        setup: ChatSetup {
            commands: vec!["tool install".into(), "tool verify".into()],
        },
        host: ChatHostSettings {
            size: Some(SessionSize {
                cpu_millis: 2_000,
                memory_mib: 4_096,
            }),
            max_threads: 12,
        },
    }
}

fn chat_settings_response() -> String {
    serde_json::json!({
        "revision": 7,
        "settings": { "chat": chat_settings() },
        "previous": { "chat": null }
    })
    .to_string()
}

#[test]
fn sdk_copies_read_every_server_owned_response_fixture() {
    let conformance: Conformance =
        serde_json::from_str(CONFORMANCE).expect("conformance fixture is typed");
    assert_eq!(conformance.version, "development");
    let session: Session = serde_json::from_str(SESSION).expect("session fixture is typed");
    assert_eq!(session.id.as_str(), "sesn_fixture");
    let chat = session
        .chat
        .expect("fixture carries the shared chat projection");
    assert_eq!(chat.worktree_name, "chat-3");
    assert_eq!(chat.worktree_path, "/workspace/chat-3");
    assert_eq!(chat.title.as_deref(), Some("Fixture chat"));
    assert_eq!(
        chat.paused_notice.as_deref(),
        Some("Paused after inactivity")
    );
    let sessions: Vec<Session> = serde_json::from_str(SESSIONS).expect("list fixture is typed");
    assert_eq!(sessions.len(), 1);
    let event: Event = serde_json::from_str(EVENT).expect("event fixture is typed");
    assert!(matches!(event.kind, EventKind::ApprovalRequested { .. }));
    let screen: ScreenSnapshot = serde_json::from_str(SCREEN).expect("screen fixture is typed");
    assert_eq!(screen.lines.len(), 3);
    let usage: Usage = serde_json::from_str(USAGE).expect("usage fixture is typed");
    assert_eq!(usage.context_window, Some(200_000));
    let preview: TriggerPreview =
        serde_json::from_str(TRIGGER_PREVIEW).expect("Trigger preview fixture is typed");
    assert_eq!(preview.repository_id, 10);
    assert_eq!(preview.availability, TriggerPreviewAvailability::Published);
    assert_eq!(preview.unavailable_reason, None);
    assert!(matches!(
        preview.estimated_cost,
        TriggerPreviewCost::ExactZero {
            amount_micros: 0,
            ..
        }
    ));
    let mut contributor_preview = fixture(TRIGGER_PREVIEW);
    contributor_preview["estimated_cost"] = serde_json::json!({
        "kind": "contributor_estimate",
        "currency": "USD",
        "amount_micros": 25_000,
        "display": "$0.025",
        "quoted_at": "2026-08-26T12:00:00Z"
    });
    assert!(matches!(
        serde_json::from_value::<TriggerPreview>(contributor_preview)
            .expect("contributor estimate is typed")
            .estimated_cost,
        TriggerPreviewCost::ContributorEstimate {
            amount_micros: 25_000,
            ..
        }
    ));
    let mut unavailable_preview = fixture(TRIGGER_PREVIEW);
    unavailable_preview["estimated_cost"] = serde_json::json!({ "kind": "quote_unavailable" });
    assert!(matches!(
        serde_json::from_value::<TriggerPreview>(unavailable_preview)
            .expect("unavailable quote is typed")
            .estimated_cost,
        TriggerPreviewCost::QuoteUnavailable
    ));
    let mut unknown_cost = fixture(TRIGGER_PREVIEW);
    unknown_cost["estimated_cost"] = serde_json::json!({ "kind": "future_quote" });
    assert!(serde_json::from_value::<TriggerPreview>(unknown_cost).is_err());
    let mut unavailable = fixture(TRIGGER_PREVIEW);
    unavailable["availability"] = serde_json::json!("unavailable");
    unavailable["unavailable_reason"] = serde_json::json!("upstream_not_forkable");
    let unavailable: TriggerPreview =
        serde_json::from_value(unavailable).expect("dynamic unavailable preview is typed");
    assert_eq!(
        unavailable.unavailable_reason,
        Some(TriggerPreviewUnavailableReason::UpstreamNotForkable)
    );
    for (availability, reason) in [
        ("published", serde_json::json!("upstream_not_forkable")),
        ("retired", serde_json::json!("upstream_unavailable")),
        ("unavailable", serde_json::Value::Null),
        ("unavailable", serde_json::json!("future_reason")),
    ] {
        let mut malformed = fixture(TRIGGER_PREVIEW);
        malformed["availability"] = serde_json::json!(availability);
        malformed["unavailable_reason"] = reason;
        assert!(serde_json::from_value::<TriggerPreview>(malformed).is_err());
    }
    for malformed in [
        serde_json::json!({
            "kind": "exact_zero",
            "currency": "EUR",
            "amount_micros": 0,
            "display": "EUR 0.00"
        }),
        serde_json::json!({
            "kind": "exact_zero",
            "currency": "USD",
            "amount_micros": 1,
            "display": "$0.00"
        }),
        serde_json::json!({
            "kind": "contributor_estimate",
            "currency": "EUR",
            "amount_micros": 25_000,
            "display": "EUR 0.025",
            "quoted_at": "2026-08-26T12:00:00Z"
        }),
    ] {
        let mut preview = fixture(TRIGGER_PREVIEW);
        preview["estimated_cost"] = malformed;
        assert!(serde_json::from_value::<TriggerPreview>(preview).is_err());
    }
    assert_eq!(
        serde_json::from_str::<LaunchConfirmationAction>(r#""complete""#)
            .expect("complete is a closed confirmation action"),
        LaunchConfirmationAction::Complete
    );
    let repository: TriggerRepositorySelection =
        serde_json::from_str(REPOSITORY_SELECTION).expect("repository selection fixture is typed");
    assert_eq!(repository.repository.repository_node_id, "R_upstream");
    assert_eq!(repository.source_sha, "a".repeat(40));
    let launch: Launch = serde_json::from_str(LAUNCH).expect("Launch fixture is typed");
    assert_eq!(launch.steps.len(), 15);
    let mut retired_launch = fixture(LAUNCH);
    retired_launch["session_credential_retirement"] = serde_json::json!({
        "session_id": "sesn_provider_smoke",
        "settled_at": "2026-08-28T01:02:45Z"
    });
    let retired_launch: Launch =
        serde_json::from_value(retired_launch).expect("credential retirement is public evidence");
    assert_eq!(
        retired_launch
            .session_credential_retirement
            .as_ref()
            .map(|retirement| retirement.session_id.as_str()),
        Some("sesn_provider_smoke")
    );
    assert_eq!(
        launch
            .runtime_admission
            .as_ref()
            .map(|runtime| runtime.kernel),
        Some(cma_sdk::TriggerKernel::CodexTriggerNativeV1)
    );
    assert!(cma_sdk::TriggerKernel::CodexTriggerNativeV1.is_current());
    assert!(!cma_sdk::TriggerKernel::CodexTriggerNativeV1.is_legacy_read_only());
    let mut legacy_kernel = fixture(LAUNCH);
    legacy_kernel["runtime_admission"]["kernel"] = serde_json::json!("codex");
    let legacy_kernel: Launch = serde_json::from_value(legacy_kernel)
        .expect("the exact historical Trigger kernel remains decodable");
    let legacy_kernel = legacy_kernel
        .runtime_admission
        .expect("the historical Launch retains its admission snapshot")
        .kernel;
    assert_eq!(legacy_kernel, cma_sdk::TriggerKernel::Codex);
    assert!(legacy_kernel.is_legacy_read_only());
    assert!(!legacy_kernel.is_current());
    let mut unknown_kernel = fixture(LAUNCH);
    unknown_kernel["runtime_admission"]["kernel"] = serde_json::json!("future_kernel");
    assert!(serde_json::from_value::<Launch>(unknown_kernel).is_err());
    let progress: LaunchProgress =
        serde_json::from_str(LAUNCH_PROGRESS).expect("Launch progress fixture is typed");
    assert_eq!(progress.next_cursor, 5);
    assert_eq!(progress.events[0].progress.state, LaunchState::Draft);
    assert!(matches!(
        &progress.events[0].kind,
        LaunchEventKind::Created { steps } if steps.len() == 15
    ));
    let review: DiffReview =
        serde_json::from_str(DIFF_REVIEW).expect("diff-review fixture is typed");
    let reviewed_files = &review
        .artifact
        .as_ref()
        .expect("fixture has an artifact")
        .files;
    assert_eq!(reviewed_files.len(), 1);
    assert_eq!(reviewed_files[0].additions, Some(1));
    assert_eq!(reviewed_files[0].deletions, Some(0));

    let mut missing_stats = fixture(DIFF_REVIEW);
    missing_stats["artifact"]["files"][0]
        .as_object_mut()
        .expect("fixture file is an object")
        .remove("additions");
    assert!(serde_json::from_value::<DiffReview>(missing_stats).is_err());
    for (binary, additions, deletions) in [
        (false, serde_json::Value::Null, serde_json::json!(0)),
        (false, serde_json::Value::Null, serde_json::Value::Null),
        (true, serde_json::json!(0), serde_json::json!(0)),
    ] {
        let mut malformed = fixture(DIFF_REVIEW);
        malformed["artifact"]["files"][0]["binary"] = serde_json::json!(binary);
        malformed["artifact"]["files"][0]["additions"] = additions;
        malformed["artifact"]["files"][0]["deletions"] = deletions;
        assert!(serde_json::from_value::<DiffReview>(malformed).is_err());
    }
    let mut binary = fixture(DIFF_REVIEW);
    binary["artifact"]["files"][0]["binary"] = serde_json::json!(true);
    binary["artifact"]["files"][0]["additions"] = serde_json::Value::Null;
    binary["artifact"]["files"][0]["deletions"] = serde_json::Value::Null;
    let binary: DiffReview =
        serde_json::from_value(binary).expect("binary line statistics are explicitly unavailable");
    let binary_artifact = binary.artifact.expect("fixture has an artifact");
    let binary_file = &binary_artifact.files[0];
    let binary_wire = serde_json::to_value(binary_file).expect("DiffFile serializes");
    assert_eq!(binary_wire["additions"], serde_json::Value::Null);
    assert_eq!(binary_wire["deletions"], serde_json::Value::Null);
    let pull_request = review
        .pull_request
        .as_ref()
        .expect("diff-review fixture has an observed pull request");
    assert_eq!(
        pull_request.provider_request_id.as_deref(),
        Some("github-request-fixture")
    );
    assert_eq!(pull_request.head_matches_launch, Some(false));
    assert_eq!(
        pull_request
            .status_failure
            .as_ref()
            .map(|failure| failure.code.as_str()),
        Some("github_status_unavailable")
    );
    assert!(matches!(
        progress.events[3].kind,
        LaunchEventKind::PullRequestStatusRefreshRequested
    ));
    assert!(matches!(
        progress.events[4].kind,
        LaunchEventKind::PullRequestStatusChanged { .. }
    ));
    let draft: PullRequestDraft =
        serde_json::from_str(PULL_REQUEST_DRAFT).expect("pull-request draft fixture is typed");
    assert_eq!(draft.operation, PullRequestOperation::Open);
    let start: LaunchStartRequest =
        serde_json::from_str(LAUNCH_START_REQUEST).expect("Start request fixture is typed");
    assert!(!start.acknowledge_workflow_risk);
    let pr_review: PullRequestReviewRequest = serde_json::from_str(PULL_REQUEST_REVIEW_REQUEST)
        .expect("pull-request review fixture is typed");
    assert_eq!(pr_review.operation, PullRequestOperation::Open);
    let pr_confirm: PullRequestConfirmRequest = serde_json::from_str(PULL_REQUEST_CONFIRM_REQUEST)
        .expect("pull-request confirmation fixture is typed");
    assert_eq!(pr_confirm.expected_revision, 0);
    let pr_refresh: PullRequestStatusRefreshRequest =
        serde_json::from_str(PULL_REQUEST_STATUS_REFRESH_REQUEST)
            .expect("pull-request status refresh fixture is typed");
    assert_eq!(pr_refresh.expected_revision, 0);
}

#[tokio::test]
async fn conformance_discovery_is_typed_anonymous_and_correlated() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, CONFORMANCE)
        .header("x-request-id", "response-conformance-488")])
    .await;
    let observed = Arc::new(StdMutex::new(Vec::<ResponseMetadata>::new()));
    let observer_values = Arc::clone(&observed);
    let credential_calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&credential_calls);
    let client = Client::builder(&stub.base_url)
        .expect("stub URL is valid")
        .credential_provider(move || {
            provider_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>("must-not-leave-the-client".to_owned())
        })
        .request_id_provider(|| Ok::<_, Infallible>("caller-conformance-488".to_owned()))
        .response_observer(move |metadata| {
            observer_values
                .lock()
                .expect("observer values remain available")
                .push(metadata.clone());
            Ok::<_, Infallible>(())
        })
        .build()
        .expect("anonymous client builds");

    let conformance = client
        .read_conformance()
        .await
        .expect("anonymous discovery succeeds");

    assert_eq!(
        conformance.conforms_to,
        ["https://spec.chrono-ai.fun/cma/1.0/conf/rest-core"]
    );
    assert_eq!(conformance.version, "development");
    assert_eq!(conformance.build_sha, "development");
    assert_eq!(
        *observed.lock().expect("observer values remain available"),
        [ResponseMetadata {
            status: 200,
            request_id: Some("response-conformance-488".to_owned()),
        }]
    );
    let requests = stub.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::GET);
    assert_eq!(requests[0].uri.path(), "/proxy/api/v1/conformance");
    assert_eq!(requests[0].authorization, None);
    assert_eq!(credential_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        requests[0].request_id.as_deref(),
        Some("caller-conformance-488")
    );
}

#[tokio::test]
async fn chat_settings_read_and_patch_use_the_revisioned_public_admin_route() {
    let body = chat_settings_response();
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, &body),
        StubResponse::json(StatusCode::OK, &body),
    ])
    .await;
    let (client, credential_calls) = stub_client(&stub, 1);

    let read = client
        .read_chat_settings()
        .await
        .expect("chat settings read succeeds");
    assert_eq!(read.revision, 7);
    assert_eq!(read.chat, Some(chat_settings()));
    assert_eq!(read.previous_chat, None);

    let changed = client
        .change_chat_settings(6, Some(&chat_settings()))
        .await
        .expect("chat settings change succeeds");
    assert_eq!(changed.chat, Some(chat_settings()));
    assert_eq!(credential_calls.load(Ordering::SeqCst), 2);

    let requests = stub.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, Method::GET);
    assert_eq!(requests[0].uri.path(), "/proxy/api/v1/admin/settings");
    assert_eq!(requests[1].method, Method::PATCH);
    assert_eq!(requests[1].uri.path(), "/proxy/api/v1/admin/settings");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[1].body).expect("PATCH body is JSON"),
        serde_json::json!({
            "expected_revision": 6,
            "chat": chat_settings(),
        })
    );
}

#[tokio::test]
async fn invalid_chat_settings_stop_before_transport_and_null_removes_the_object() {
    let body = serde_json::json!({
        "revision": 8,
        "settings": { "chat": null },
        "previous": { "chat": chat_settings() }
    })
    .to_string();
    let stub = Stub::start([StubResponse::json(StatusCode::OK, &body)]).await;
    let (client, credential_calls) = stub_client(&stub, 1);

    let mut invalid = chat_settings();
    invalid.host.max_threads = 0;
    assert!(
        client
            .change_chat_settings(7, Some(&invalid))
            .await
            .is_err()
    );
    assert!(stub.requests().await.is_empty());
    assert_eq!(credential_calls.load(Ordering::SeqCst), 0);

    let removed = client
        .change_chat_settings(7, None)
        .await
        .expect("null removes chat");
    assert_eq!(removed.chat, None);
    assert_eq!(removed.previous_chat, Some(chat_settings()));
    let requests = stub.requests().await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0].body).unwrap(),
        serde_json::json!({ "expected_revision": 7, "chat": null })
    );
}

#[tokio::test]
async fn trigger_preview_is_typed_and_always_anonymous() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, TRIGGER_PREVIEW)]).await;
    let (client, credential_calls) = stub_client(&stub, 1);

    let preview = client
        .read_trigger_preview("33333333-3333-4333-8333-333333333333")
        .await
        .expect("public Trigger preview succeeds");

    assert_eq!(preview.repository_id, 10);
    assert_eq!(credential_calls.load(Ordering::SeqCst), 0);
    let requests = stub.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].uri.path(),
        "/proxy/api/v1/triggers/public/33333333-3333-4333-8333-333333333333"
    );
    assert_eq!(requests[0].authorization, None);
    assert_eq!(requests[0].idempotency_key, None);
}

#[tokio::test]
async fn every_trigger_command_uses_the_exact_key_and_canonical_path() {
    let draft = trigger_draft_response();
    let revision = trigger_revision_response();
    let authorization = connection_authorization_response();
    let connection = trigger_connection_response();
    let revoked_connection = revoked_trigger_connection_response();
    let stub = Stub::start([
        StubResponse::json(StatusCode::CREATED, &draft),
        StubResponse::json(StatusCode::OK, &draft),
        StubResponse::json(StatusCode::CREATED, &revision),
        StubResponse::json(StatusCode::OK, &revision),
        StubResponse::json(StatusCode::OK, &authorization),
        StubResponse::json(StatusCode::OK, &authorization),
        StubResponse::json(StatusCode::OK, &revoked_connection),
        StubResponse::json(StatusCode::OK, &connection),
        StubResponse::json(StatusCode::CREATED, LAUNCH),
        StubResponse::json(StatusCode::CREATED, LAUNCH),
        StubResponse::json(StatusCode::OK, LAUNCH),
        StubResponse::json(StatusCode::OK, LAUNCH),
        StubResponse::json(StatusCode::ACCEPTED, LAUNCH),
        StubResponse::json(StatusCode::ACCEPTED, LAUNCH),
        StubResponse::json(StatusCode::ACCEPTED, LAUNCH),
        StubResponse::json(StatusCode::ACCEPTED, LAUNCH),
        StubResponse::json(StatusCode::ACCEPTED, LAUNCH),
        StubResponse::json(StatusCode::ACCEPTED, LAUNCH),
        StubResponse::json(StatusCode::OK, LAUNCH),
        StubResponse::json(StatusCode::OK, LAUNCH),
        StubResponse::json(StatusCode::ACCEPTED, LAUNCH),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);
    let content = trigger_content_draft();

    let _: TriggerDraft = client
        .create_trigger(
            CreateTriggerRequest {
                selected_service_id: "service_fixture".into(),
                content: content.clone(),
            },
            "exact-00",
        )
        .await
        .unwrap();
    let _: TriggerDraft = client
        .update_trigger(
            "trigger_fixture",
            UpdateTriggerRequest {
                expected_version: 1,
                selected_service_id: "service_fixture".into(),
                content,
            },
            "exact-01",
        )
        .await
        .unwrap();
    let _: TriggerRevision = client
        .publish_trigger(
            "trigger_fixture",
            PublishTriggerRequest {
                expected_version: 1,
                selected_service_id: "service_fixture".into(),
            },
            "exact-02",
        )
        .await
        .unwrap();
    client
        .retire_trigger_revision("revision_fixture", "exact-03")
        .await
        .unwrap();
    client
        .begin_trigger_connection(
            BeginConnectionRequest {
                intent_id: "intent_fixture".into(),
            },
            "exact-04",
        )
        .await
        .unwrap();
    client
        .reconnect_trigger_connection("service_fixture", "exact-05")
        .await
        .unwrap();
    let revoked: TriggerConnection = client
        .revoke_trigger_connection("service_fixture", "exact-06")
        .await
        .unwrap();
    assert_eq!(revoked.state, TriggerConnectionState::Inactive);
    let _: TriggerConnection = client
        .preflight_trigger_connection("service_fixture")
        .await
        .unwrap();
    client
        .create_or_resume_launch("public_fixture", "exact-07")
        .await
        .unwrap();
    client
        .create_new_launch("public_fixture", "exact-08")
        .await
        .unwrap();
    client
        .update_launch_overrides(
            "launch_fixture",
            UpdateLaunchOverridesRequest {
                expected_revision: 0,
                overrides: Default::default(),
            },
            "exact-09",
        )
        .await
        .unwrap();
    client
        .select_launch_connection(
            "launch_fixture",
            SelectLaunchConnectionRequest {
                expected_revision: 0,
                service_id: "service_fixture".into(),
            },
            "exact-10",
        )
        .await
        .unwrap();
    client
        .start_launch(
            "launch_fixture",
            serde_json::from_str(LAUNCH_START_REQUEST).unwrap(),
            "exact-11",
        )
        .await
        .unwrap();
    let confirmation = LaunchConfirmationRequest {
        expected_revision: 0,
        snapshot_digest: "snapshot_fixture".into(),
    };
    client
        .request_launch_diff("launch_fixture", confirmation.clone(), "exact-12")
        .await
        .unwrap();
    client
        .publish_launch_branch("launch_fixture", confirmation.clone(), "exact-13")
        .await
        .unwrap();
    client
        .confirm_pull_request(
            "launch_fixture",
            PullRequestOperation::Open,
            serde_json::from_str(PULL_REQUEST_CONFIRM_REQUEST).unwrap(),
            "exact-14",
        )
        .await
        .unwrap();
    client
        .confirm_pull_request(
            "launch_fixture",
            PullRequestOperation::Update,
            serde_json::from_str(PULL_REQUEST_CONFIRM_REQUEST).unwrap(),
            "exact-15",
        )
        .await
        .unwrap();
    client
        .refresh_pull_request_status(
            "launch_fixture",
            serde_json::from_str(PULL_REQUEST_STATUS_REFRESH_REQUEST).unwrap(),
            "exact-16",
        )
        .await
        .unwrap();
    client
        .cancel_launch("launch_fixture", confirmation.clone(), "exact-17")
        .await
        .unwrap();
    client
        .complete_launch("launch_fixture", confirmation, "exact-18")
        .await
        .unwrap();
    client
        .recover_initial_prompt(
            "launch_fixture",
            RecoverInitialPromptRequest {
                expected_revision: 12,
                snapshot_digest: "retained-payload-hash".into(),
                acknowledge_duplicate_execution_risk: true,
            },
            "exact-19",
        )
        .await
        .unwrap();

    let requests = stub.requests().await;
    let expected = [
        (Method::POST, "/proxy/api/v1/triggers"),
        (Method::PUT, "/proxy/api/v1/triggers/trigger_fixture"),
        (
            Method::POST,
            "/proxy/api/v1/triggers/trigger_fixture/publish",
        ),
        (
            Method::POST,
            "/proxy/api/v1/trigger-revisions/revision_fixture/retire",
        ),
        (Method::POST, "/proxy/api/v1/trigger-connections"),
        (
            Method::POST,
            "/proxy/api/v1/trigger-connections/service_fixture/reconnect",
        ),
        (
            Method::DELETE,
            "/proxy/api/v1/trigger-connections/service_fixture",
        ),
        (
            Method::GET,
            "/proxy/api/v1/trigger-connections/service_fixture/preflight",
        ),
        (
            Method::POST,
            "/proxy/api/v1/triggers/public/public_fixture/launches",
        ),
        (
            Method::POST,
            "/proxy/api/v1/triggers/public/public_fixture/launches/new",
        ),
        (
            Method::PATCH,
            "/proxy/api/v1/launches/launch_fixture/overrides",
        ),
        (
            Method::PUT,
            "/proxy/api/v1/launches/launch_fixture/connection",
        ),
        (Method::POST, "/proxy/api/v1/launches/launch_fixture/start"),
        (
            Method::POST,
            "/proxy/api/v1/launches/launch_fixture/contributions/review",
        ),
        (
            Method::POST,
            "/proxy/api/v1/launches/launch_fixture/contributions/publish",
        ),
        (
            Method::POST,
            "/proxy/api/v1/launches/launch_fixture/pull-request/open",
        ),
        (
            Method::POST,
            "/proxy/api/v1/launches/launch_fixture/pull-request/update",
        ),
        (
            Method::POST,
            "/proxy/api/v1/launches/launch_fixture/pull-request/refresh",
        ),
        (Method::POST, "/proxy/api/v1/launches/launch_fixture/cancel"),
        (
            Method::POST,
            "/proxy/api/v1/launches/launch_fixture/complete",
        ),
        (
            Method::POST,
            "/proxy/api/v1/launches/launch_fixture/recover-initial-prompt",
        ),
    ];
    assert_eq!(requests.len(), expected.len());
    let keys = [
        Some("exact-00"),
        Some("exact-01"),
        Some("exact-02"),
        Some("exact-03"),
        Some("exact-04"),
        Some("exact-05"),
        Some("exact-06"),
        None,
        Some("exact-07"),
        Some("exact-08"),
        Some("exact-09"),
        Some("exact-10"),
        Some("exact-11"),
        Some("exact-12"),
        Some("exact-13"),
        Some("exact-14"),
        Some("exact-15"),
        Some("exact-16"),
        Some("exact-17"),
        Some("exact-18"),
        Some("exact-19"),
    ];
    for (index, ((request, (method, path)), key)) in
        requests.iter().zip(expected).zip(keys).enumerate()
    {
        assert_eq!(request.method, method, "command {index}");
        assert_eq!(request.uri.path(), path, "command {index}");
        assert_eq!(request.idempotency_key.as_deref(), key, "command {index}");
    }
    assert_eq!(
        fixture(std::str::from_utf8(&requests[12].body).unwrap()),
        fixture(LAUNCH_START_REQUEST)
    );
    assert_eq!(
        fixture(std::str::from_utf8(&requests[17].body).unwrap()),
        fixture(PULL_REQUEST_STATUS_REFRESH_REQUEST)
    );
    assert_eq!(
        fixture(std::str::from_utf8(&requests[19].body).unwrap()),
        serde_json::json!({
            "expected_revision": 0,
            "snapshot_digest": "snapshot_fixture",
        })
    );
    assert_eq!(
        fixture(std::str::from_utf8(&requests[20].body).unwrap()),
        serde_json::json!({
            "expected_revision": 12,
            "snapshot_digest": "retained-payload-hash",
            "acknowledge_duplicate_execution_risk": true,
        })
    );
}

#[tokio::test]
async fn trigger_connection_revocation_rejects_a_different_service_projection() {
    let mut connection = fixture(&trigger_connection_response());
    connection["service_id"] = serde_json::json!("service_other");
    let stub = Stub::start([StubResponse::json(StatusCode::OK, &connection.to_string())]).await;
    let (client, _) = stub_client(&stub, 1);

    assert!(matches!(
        client
            .revoke_trigger_connection("service_fixture", "revoke-exact-key")
            .await,
        Err(Error::Protocol { ref message })
            if message.contains("different GitHub connection")
    ));
}

#[tokio::test]
async fn repository_resolution_is_authenticated_keyless_and_canonical() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, REPOSITORY_SELECTION)]).await;
    let (client, _) = stub_client(&stub, 1);

    let selection = client
        .resolve_trigger_repository("service_fixture", "upstream-owner", ".github")
        .await
        .unwrap();
    assert_eq!(selection.repository.repository_id, 10);

    let requests = stub.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, Method::GET);
    assert_eq!(
        requests[0].uri.path(),
        "/proxy/api/v1/trigger-connections/service_fixture/repositories/resolve"
    );
    assert_eq!(
        requests[0].uri.query(),
        Some("owner=upstream-owner&name=.github")
    );
    assert!(requests[0].authorization.is_some());
    assert_eq!(requests[0].idempotency_key, None);
}

#[tokio::test]
async fn repository_resolution_rejects_non_canonical_locators_before_transport() {
    let stub = Stub::start([]).await;
    let (client, _) = stub_client(&stub, 1);

    for (owner, name) in [
        ("", "project"),
        ("upstream/owner", "project"),
        ("upstream-owner", ".."),
        ("upstream-owner", "project?private=true"),
        (" upstream-owner", "project"),
    ] {
        assert!(matches!(
            client
                .resolve_trigger_repository("service_fixture", owner, name)
                .await,
            Err(Error::Protocol { ref message })
                if message.contains("canonical owner or repository name")
        ));
    }
    assert!(stub.requests().await.is_empty());
}

#[tokio::test]
async fn pull_request_review_is_keyless_while_confirmation_is_keyed() {
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, PULL_REQUEST_DRAFT),
        StubResponse::json(StatusCode::ACCEPTED, LAUNCH),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);

    client
        .review_pull_request(
            "launch_fixture",
            serde_json::from_str(PULL_REQUEST_REVIEW_REQUEST).unwrap(),
        )
        .await
        .unwrap();
    client
        .confirm_pull_request(
            "launch_fixture",
            PullRequestOperation::Open,
            serde_json::from_str(PULL_REQUEST_CONFIRM_REQUEST).unwrap(),
            "confirm-exact",
        )
        .await
        .unwrap();

    let requests = stub.requests().await;
    assert_eq!(requests[0].idempotency_key, None);
    assert_eq!(
        requests[1].idempotency_key.as_deref(),
        Some("confirm-exact")
    );
}

#[tokio::test]
async fn trigger_contract_rejects_internal_fields_before_typed_decoding() {
    let forbidden = [
        "idempotency_key_hash",
        "intent_key",
        "provider_request_id",
        "object_reference",
        "baseline_object_reference",
        "backing_key_id",
        "worker_fencing_token",
        "fencing_counter",
        "subject_token_generation",
        "session_credential_generation",
        "session_credential_authority",
        "custody_generation",
        "provider_key_id",
        "provider_state_version",
        "grant_fingerprint",
        "checkpoint_object",
        "diff_artifact_object",
        "prompt_origin_key",
        "prompt_payload_hash",
    ];
    let responses = forbidden.iter().map(|field| {
        let mut launch = fixture(LAUNCH);
        launch["steps"][0][*field] = serde_json::json!(99);
        StubResponse::json(StatusCode::OK, &launch.to_string())
    });
    let stub = Stub::start(responses).await;
    let (client, _) = stub_client(&stub, 1);

    for field in forbidden {
        assert!(matches!(
            client.read_launch("launch_fixture").await,
            Err(Error::Protocol { ref message }) if message.contains(field)
        ));
    }
}

#[tokio::test]
async fn trigger_contract_admits_public_pull_request_provider_correlation_at_its_record() {
    let mut launch = fixture(LAUNCH);
    let review = fixture(DIFF_REVIEW);
    launch["state"] = serde_json::json!("pr_open");
    launch["pull_request"] = review["pull_request"].clone();
    let page = serde_json::json!({ "items": [launch.clone()], "next_cursor": null });
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, &launch.to_string()),
        StubResponse::json(StatusCode::OK, &page.to_string()),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);

    let launch = client.read_launch("launch_fixture").await.unwrap();
    let pull_request = launch.pull_request.expect("fixture carries a pull request");
    assert_eq!(
        pull_request.provider_request_id.as_deref(),
        Some("github-request-fixture")
    );
    assert_eq!(pull_request.head_matches_launch, Some(false));
    let page = client
        .list_launches(LaunchListRequest::default())
        .await
        .unwrap();
    assert_eq!(
        page.items[0]
            .pull_request
            .as_ref()
            .and_then(|record| record.provider_request_id.as_deref()),
        Some("github-request-fixture")
    );
}

#[tokio::test]
async fn launch_list_filters_by_one_validated_session_identifier() {
    let page = serde_json::json!({ "items": [], "next_cursor": null });
    let stub = Stub::start([StubResponse::json(StatusCode::OK, &page.to_string())]).await;
    let (client, _) = stub_client(&stub, 1);

    client
        .list_launches(LaunchListRequest {
            session_id: Some("ses_fixture".to_owned()),
            limit: Some(1),
            ..LaunchListRequest::default()
        })
        .await
        .unwrap();

    let requests = stub.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].uri.path(), "/proxy/api/v1/launches");
    assert_eq!(
        requests[0].uri.query(),
        Some("session_id=ses_fixture&limit=1")
    );
}

#[tokio::test]
async fn launch_list_rejects_invalid_session_identifiers_before_transport() {
    let stub = Stub::start([]).await;
    let (client, _) = stub_client(&stub, 1);

    for session_id in ["", "   ", "ses/fixture"] {
        assert!(matches!(
            client
                .list_launches(LaunchListRequest {
                    session_id: Some(session_id.to_owned()),
                    ..LaunchListRequest::default()
                })
                .await,
            Err(Error::Protocol { ref message })
                if message.contains("session id must be a non-empty path identifier")
        ));
    }
    assert!(stub.requests().await.is_empty());
}

#[tokio::test]
async fn trigger_contract_rejects_unknown_closed_enum_values() {
    let mut launch = fixture(LAUNCH);
    launch["effective"]["permission"] = serde_json::json!("future_permission");
    let stub = Stub::start([StubResponse::json(StatusCode::OK, &launch.to_string())]).await;
    let (client, _) = stub_client(&stub, 1);

    assert!(matches!(
        client.read_launch("launch_fixture").await,
        Err(Error::Protocol { ref message })
            if message.contains("future_permission") && message.contains("unknown variant")
    ));
}

#[tokio::test]
async fn launch_progress_normalizes_identical_duplicates_and_rejects_unsafe_replay() {
    let base = fixture(LAUNCH_PROGRESS);
    let launch_id = base["launch_id"]
        .as_str()
        .expect("progress fixture has a Launch id")
        .to_owned();

    let mut identical = base.clone();
    let duplicate = identical["events"][0].clone();
    identical["events"]
        .as_array_mut()
        .unwrap()
        .insert(1, duplicate);

    let mut conflicting = base.clone();
    let mut duplicate = conflicting["events"][0].clone();
    duplicate["correlation_id"] = serde_json::json!("conflicting-correlation");
    conflicting["events"]
        .as_array_mut()
        .unwrap()
        .insert(1, duplicate);

    let mut gap = base.clone();
    gap["events"].as_array_mut().unwrap().remove(1);

    let mut mixed_launch = base.clone();
    mixed_launch["events"][1]["launch_id"] = serde_json::json!("launch_other");

    let mut bad_cursor = base.clone();
    bad_cursor["next_cursor"] = serde_json::json!(4);

    let mut wrong_page = base.clone();
    wrong_page["launch_id"] = serde_json::json!("launch_other");

    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, &identical.to_string()),
        StubResponse::json(StatusCode::OK, &conflicting.to_string()),
        StubResponse::json(StatusCode::OK, &gap.to_string()),
        StubResponse::json(StatusCode::OK, &mixed_launch.to_string()),
        StubResponse::json(StatusCode::OK, &bad_cursor.to_string()),
        StubResponse::json(StatusCode::OK, &wrong_page.to_string()),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);

    let progress = client
        .read_launch_progress(&launch_id, 0, 100)
        .await
        .expect("an identical adjacent duplicate is harmless");
    assert_eq!(progress.events.len(), 5);
    assert_eq!(progress.next_cursor, 5);

    for expected in [
        "conflicting duplicate",
        "sequence gap",
        "belongs to launch_other",
        "next_cursor 4",
        "response belongs to launch_other",
    ] {
        assert!(matches!(
            client.read_launch_progress(&launch_id, 0, 500).await,
            Err(Error::Protocol { ref message }) if message.contains(expected)
        ));
    }

    assert!(matches!(
        client.read_launch_progress(&launch_id, 0, 501).await,
        Err(Error::Protocol { ref message }) if message.contains("1 through 500")
    ));
    assert_eq!(stub.requests().await.len(), 6);
}

#[tokio::test]
async fn response_observation_precedes_strict_success_decoding() {
    let with_unknown_field =
        CONFORMANCE.replace("\n}", ",\n  \"unexpected\": \"must be rejected\"\n}");
    let stub = Stub::start([StubResponse::json(StatusCode::OK, &with_unknown_field)
        .header("x-request-id", "response-before-decode")])
    .await;
    let observed = Arc::new(AtomicUsize::new(0));
    let observer_calls = Arc::clone(&observed);
    let client = Client::builder(&stub.base_url)
        .expect("stub URL is valid")
        .response_observer(move |metadata| {
            assert_eq!(metadata.status, 200);
            assert_eq!(
                metadata.request_id.as_deref(),
                Some("response-before-decode")
            );
            observer_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>(())
        })
        .build()
        .expect("anonymous client builds");

    assert!(matches!(
        client.read_conformance().await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(observed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn missing_blank_and_opaque_response_ids_are_absent_metadata() {
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, CONFORMANCE),
        StubResponse::json(StatusCode::OK, CONFORMANCE).raw_header("x-request-id", b" \t "),
        StubResponse::json(StatusCode::OK, CONFORMANCE).raw_header("x-request-id", &[0xff]),
    ])
    .await;
    let observed = Arc::new(StdMutex::new(Vec::<ResponseMetadata>::new()));
    let observer_values = Arc::clone(&observed);
    let client = Client::builder(&stub.base_url)
        .expect("stub URL is valid")
        .response_observer(move |metadata| {
            observer_values
                .lock()
                .expect("observer values remain available")
                .push(metadata.clone());
            Ok::<_, Infallible>(())
        })
        .build()
        .expect("anonymous client builds");

    for _ in 0..3 {
        client
            .read_conformance()
            .await
            .expect("an unusable correlation header does not damage the body");
    }
    let observed = observed.lock().expect("observer values remain available");
    assert_eq!(observed.len(), 3);
    assert!(
        observed
            .iter()
            .all(|metadata| { metadata.status == 200 && metadata.request_id.is_none() })
    );
}

#[tokio::test]
async fn observer_failure_is_explicit_and_stops_response_handling() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, CONFORMANCE)]).await;
    let client = Client::builder(&stub.base_url)
        .expect("stub URL is valid")
        .response_observer(|_| Err::<(), _>("metadata sink unavailable"))
        .build()
        .expect("anonymous client builds");

    let error = client
        .read_conformance()
        .await
        .expect_err("observer refusal wins before body decoding");
    assert!(matches!(
        error,
        Error::Observer { ref message } if message.contains("metadata sink unavailable")
    ));
    assert_eq!(stub.requests().await.len(), 1);
}

#[tokio::test]
async fn observer_panic_is_contained_as_an_explicit_sdk_error() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, CONFORMANCE)]).await;
    let client = Client::builder(&stub.base_url)
        .expect("stub URL is valid")
        .response_observer(|_| -> Result<(), Infallible> {
            panic!("observer fixture panic");
        })
        .build()
        .expect("anonymous client builds");

    assert!(matches!(
        client.read_conformance().await,
        Err(Error::Observer { ref message }) if message == "response observer panicked"
    ));
    assert_eq!(stub.requests().await.len(), 1);
}

#[tokio::test]
async fn unusable_or_failed_request_id_providers_stop_before_http() {
    for request_id in ["", " \t ", "line\nbreak"] {
        let stub = Stub::start([StubResponse::json(StatusCode::OK, CONFORMANCE)]).await;
        let client = Client::builder(&stub.base_url)
            .expect("stub URL is valid")
            .request_id_provider(move || Ok::<_, Infallible>(request_id.to_owned()))
            .build()
            .expect("anonymous client builds");
        assert!(matches!(
            client.read_conformance().await,
            Err(Error::Protocol { .. })
        ));
        assert!(stub.requests().await.is_empty());
    }

    let stub = Stub::start([StubResponse::json(StatusCode::OK, CONFORMANCE)]).await;
    let client = Client::builder(&stub.base_url)
        .expect("stub URL is valid")
        .request_id_provider(|| Err::<String, _>("trace context unavailable"))
        .build()
        .expect("anonymous client builds");
    assert!(matches!(
        client.read_conformance().await,
        Err(Error::Transport { .. })
    ));
    assert!(stub.requests().await.is_empty());
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
            surface: None,
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
async fn chat_creation_serializes_the_closed_surface_value() {
    let stub = Stub::start([StubResponse::json(StatusCode::CREATED, SESSION)]).await;
    let (client, _) = stub_client(&stub, 1);

    let session = client
        .create_session(CreateSessionRequest {
            surface: Some(SessionSurface::Chat),
            ..CreateSessionRequest::default()
        })
        .await
        .expect("chat create succeeds");
    assert_eq!(
        session.chat.expect("chat projection").worktree_name,
        "chat-3"
    );

    let requests = stub.requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0].body).expect("request is JSON"),
        serde_json::json!({ "surface": "chat" })
    );
}

#[tokio::test]
async fn malformed_chat_projection_is_a_protocol_failure() {
    let mut malformed = fixture(SESSION);
    malformed["chat"]["worktree_path"] = serde_json::Value::String(String::new());
    let body = malformed.to_string();
    let stub = Stub::start([StubResponse::json(StatusCode::OK, &body)]).await;
    let (client, _) = stub_client(&stub, 1);
    let id = SessionId::new("sesn_fixture").expect("fixture id is valid");

    let error = client
        .read_session(&id)
        .await
        .expect_err("an empty worktree path is not a usable chat projection");
    assert!(matches!(error, Error::Protocol { .. }));
    assert!(error.to_string().contains("worktree path"));
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
            "</api/v1/sessions?status=running&surface=chat&limit=1&cursor=opaque-next>; rel=\"next\"",
        ),
        StubResponse::json(StatusCode::OK, &older),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);
    let request = SessionPageRequest::new()
        .with_status(SessionStatus::Running)
        .expect("a first-page status filter is accepted")
        .with_surface(SessionSurfaceFilter::Chat)
        .expect("a first-page surface filter is accepted")
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
    assert!(
        SessionPageRequest::continuing(next.clone())
            .with_surface(SessionSurfaceFilter::Agent)
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
            "/proxy/api/v1/sessions?status=running&surface=chat&limit=1&cursor=opaque+start",
            "/proxy/api/v1/sessions?status=running&surface=chat&limit=1&cursor=opaque-next",
        ]
    );
}

#[tokio::test]
async fn chat_host_inventory_decodes_projection_and_route_bound_pages() {
    let retired = CHAT_HOSTS
        .replace(
            "chho_0123456789abcdef0123456789abcdef",
            "chho_fedcba9876543210fedcba9876543210",
        )
        .replace("\"status\": \"active\"", "\"status\": \"retired\"");
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, CHAT_HOSTS).header(
            "link",
            "</api/v1/admin/chat/hosts?limit=1&cursor=opaque-next>; rel=\"next\"",
        ),
        StubResponse::json(StatusCode::OK, &retired),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);
    let request = ChatHostPageRequest::new()
        .with_limit(1)
        .expect("one is within the page bound")
        .with_cursor("opaque start")
        .expect("an opaque cursor is accepted");

    let first = client
        .list_chat_host_page(request)
        .await
        .expect("first host page lists");
    let host = &first.items[0];
    assert_eq!(host.id.as_str(), "chho_0123456789abcdef0123456789abcdef");
    assert_eq!(host.owner, "subject-owner");
    assert_eq!(host.status, ChatHostStatus::Active);
    assert_eq!(host.thread_count, 3);
    assert_eq!(host.runtime_version.as_deref(), Some("runtime-v2"));
    assert_eq!(host.paused_at.as_deref(), Some("2026-09-01T01:00:00Z"));
    assert_eq!(host.revived_at.as_deref(), Some("2026-09-01T02:00:00Z"));
    assert_eq!(
        host.checkpoint_captured_at.as_deref(),
        Some("2026-09-01T00:45:00Z")
    );
    assert_eq!(host.settings_revision, 17);
    assert_eq!(host.created_at, "2026-08-31T23:00:00Z");

    let next = first.next.expect("first host page continues");
    assert!(
        ChatHostPageRequest::continuing(next.clone())
            .with_limit(2)
            .is_err()
    );
    assert!(
        ChatHostPageRequest::continuing(next.clone())
            .with_cursor("replacement")
            .is_err()
    );
    let last = client
        .list_chat_host_page(ChatHostPageRequest::continuing(next))
        .await
        .expect("manual host continuation lists");
    assert_eq!(last.items[0].status, ChatHostStatus::Retired);
    assert!(last.next.is_none());

    assert_eq!(
        stub.requests()
            .await
            .iter()
            .map(|request| request.uri.to_string())
            .collect::<Vec<_>>(),
        [
            "/proxy/api/v1/admin/chat/hosts?limit=1&cursor=opaque+start",
            "/proxy/api/v1/admin/chat/hosts?limit=1&cursor=opaque-next",
        ]
    );
}

#[tokio::test]
async fn list_chat_hosts_walks_pages_with_finite_boundaries() {
    let older = CHAT_HOSTS.replace(
        "chho_0123456789abcdef0123456789abcdef",
        "chho_fedcba9876543210fedcba9876543210",
    );
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, CHAT_HOSTS).header(
            "link",
            "</api/v1/admin/chat/hosts?limit=50&cursor=older>; rel=\"next\"",
        ),
        StubResponse::json(StatusCode::OK, &older),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);

    let hosts = client
        .list_chat_hosts()
        .await
        .expect("both host pages list");
    assert_eq!(hosts.len(), 2);
    assert_eq!(
        stub.requests()
            .await
            .iter()
            .map(|request| request.uri.to_string())
            .collect::<Vec<_>>(),
        [
            "/proxy/api/v1/admin/chat/hosts",
            "/proxy/api/v1/admin/chat/hosts?limit=50&cursor=older",
        ]
    );

    let bounded = Stub::start([StubResponse::json(StatusCode::OK, CHAT_HOSTS).header(
        "link",
        "</api/v1/admin/chat/hosts?cursor=more>; rel=\"next\"",
    )])
    .await;
    let (bounded_client, _) = stub_client(&bounded, 1);
    assert!(matches!(
        bounded_client
            .list_chat_hosts_with_limits(PageWalkLimits::new(1, 10).expect("valid limits"))
            .await,
        Err(Error::Protocol { .. })
    ));
}

#[tokio::test]
async fn chat_host_inventory_refuses_cross_collection_continuations() {
    let stub = Stub::start([StubResponse::json(StatusCode::OK, CHAT_HOSTS).header(
        "link",
        "</api/v1/sessions?cursor=wrong-collection>; rel=\"next\"",
    )])
    .await;
    let (client, _) = stub_client(&stub, 1);

    assert!(matches!(
        client.list_chat_hosts().await,
        Err(Error::Protocol { .. })
    ));
    assert_eq!(stub.requests().await.len(), 1);
}

#[tokio::test]
async fn retire_chat_host_posts_and_accepts_idempotent_http_ok() {
    let retired = CHAT_HOSTS.replace("\"status\": \"active\"", "\"status\": \"retired\"");
    let stub = Stub::start([
        StubResponse::json(StatusCode::OK, &retired[1..retired.len() - 2]),
        StubResponse::json(StatusCode::OK, &retired[1..retired.len() - 2]),
    ])
    .await;
    let (client, _) = stub_client(&stub, 1);
    let id =
        ChatHostId::new("chho_0123456789abcdef0123456789abcdef").expect("fixture host id is valid");

    assert_eq!(
        client
            .retire_chat_host(&id)
            .await
            .expect("active host retires")
            .status,
        ChatHostStatus::Retired
    );
    assert_eq!(
        client
            .retire_chat_host(&id)
            .await
            .expect("retired host is an idempotent success")
            .status,
        ChatHostStatus::Retired
    );
    let requests = stub.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.method == Method::POST)
    );
    assert!(requests.iter().all(|request| {
        request.uri.path()
            == "/proxy/api/v1/admin/chat/hosts/chho_0123456789abcdef0123456789abcdef/retire"
    }));
    assert!(requests.iter().all(|request| request.body.is_empty()));
}

#[tokio::test]
async fn malformed_chat_host_projection_is_a_protocol_failure() {
    let malformed = CHAT_HOSTS.replace("\"status\": \"active\"", "\"status\": \"future\"");
    let stub = Stub::start([StubResponse::json(StatusCode::OK, &malformed)]).await;
    let (client, _) = stub_client(&stub, 1);

    assert!(matches!(
        client.list_chat_hosts().await,
        Err(Error::Protocol { .. })
    ));
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
    assert!(ChatHostPageRequest::new().with_limit(0).is_err());
    assert!(
        ChatHostPageRequest::new()
            .with_limit(MAX_PAGE_SIZE + 1)
            .is_err()
    );
    assert!(ChatHostPageRequest::new().with_cursor("").is_err());
    assert!(ChatHostId::new("session-shaped").is_err());
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
    assert_eq!(requests[1].uri.query(), Some("starting_after=7"));
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
    assert_eq!(requests[1].uri.query(), Some("starting_after=8"));
    assert_eq!(requests[1].last_event_id.as_deref(), Some("8"));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn capability_refusal_is_publicly_distinct_from_transport_failure() {
    let stub =
        Stub::start([
            StubResponse::json(StatusCode::UNPROCESSABLE_ENTITY, CAPABILITY_REFUSED)
                .header("x-request-id", "response-capability-488"),
        ])
        .await;
    let (client, _) = stub_client(&stub, 1);
    let id = SessionId::new("sesn_fixture").expect("valid id");
    let error = client
        .steer(&id, "continue")
        .await
        .expect_err("steer is refused");
    assert!(matches!(
        error,
        Error::CapabilityRefused {
            ref capability,
            request_id: Some(ref request_id),
        } if capability == "steer" && request_id == "response-capability-488"
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
async fn request_id_provider_is_resolved_once_and_reused_across_http_retries() {
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
            .header("retry-after", "0")
            .header("x-request-id", "response-retry-one"),
        StubResponse::json(StatusCode::OK, SESSIONS).header("x-request-id", "response-retry-two"),
    ])
    .await;
    let request_id_calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = Arc::clone(&request_id_calls);
    let observed = Arc::new(StdMutex::new(Vec::<ResponseMetadata>::new()));
    let observer_values = Arc::clone(&observed);
    let client = Client::builder(&stub.base_url)
        .expect("stub URL is valid")
        .credential_provider(|| Ok::<_, Infallible>("fixture-auth".to_owned()))
        .request_id_provider(move || {
            provider_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>("caller-retry-488/exact".to_owned())
        })
        .response_observer(move |metadata| {
            observer_values
                .lock()
                .expect("observer values remain available")
                .push(metadata.clone());
            Ok::<_, Infallible>(())
        })
        .retry_policy(no_delay_policy(2))
        .build()
        .expect("client builds");

    client.list_sessions().await.expect("retry succeeds");

    assert_eq!(request_id_calls.load(Ordering::SeqCst), 1);
    let requests = stub.requests().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| { request.request_id.as_deref() == Some("caller-retry-488/exact") })
    );
    assert_eq!(
        *observed.lock().expect("observer values remain available"),
        [
            ResponseMetadata {
                status: 503,
                request_id: Some("response-retry-one".to_owned()),
            },
            ResponseMetadata {
                status: 200,
                request_id: Some("response-retry-two".to_owned()),
            },
        ]
    );
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
        StubResponse::json(StatusCode::SERVICE_UNAVAILABLE, &unavailable)
            .header("x-request-id", "response-api-488"),
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
            request_id: Some(ref request_id),
            ..
        } if code == "runtime_unavailable" && request_id == "response-api-488"
    ));
    assert_eq!(stub.requests().await.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn structured_api_error_survives_a_missing_response_id() {
    let problem = serde_json::json!({
        "type": "https://spec.chrono-ai.fun/cma/1.0/problems/invalid-state",
        "title": "invalid state",
        "status": 409,
        "detail": "session is rotating",
        "code": "invalid_state"
    })
    .to_string();
    let stub = Stub::start([StubResponse::json(StatusCode::CONFLICT, &problem)]).await;
    let (client, _) = stub_client(&stub, 1);

    let error = client
        .list_sessions()
        .await
        .expect_err("the structured refusal is returned");

    assert!(matches!(
        error,
        Error::Api {
            status: 409,
            ref code,
            request_id: None,
            ..
        } if code == "invalid_state"
    ));
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
