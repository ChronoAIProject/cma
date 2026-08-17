//! REST interface — for backend server clients.
//!
//! The superset facade: it has no structural constraint, so it projects every
//! core operation and serves as the reference realisation. Errors are RFC 9457
//! problem documents, never a bare status code.

use std::sync::Arc;

use axum::{
    extract::{FromRef, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use layer2_core::{
    Caller, CoreError, KernelId, PublicIdentity, Session, SessionCapabilities, SessionId,
    SessionService, SessionSpec, TerminalGeometry, UserDirectory,
};
use serde::{Deserialize, Serialize};

mod events;

use crate::auth::{AuthenticatedCaller, DelegatedCredential};
use crate::{InterfaceProfile, Operation};

/// Base URI for problem types this interface returns.
pub(crate) const PROBLEM_BASE: &str = "https://spec.chrono-ai.fun/chrono-bot/1.0/problems";

/// Conformance classes this interface satisfies.
const CONFORMS_TO: &[&str] = &[
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/core",
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/terminal",
];

/// Where every conformance class this service declares is rooted.
const CONF_BASE: &str = "https://spec.chrono-ai.fun/chrono-bot/1.0/conf";

/// The classes one session satisfies, given what it was granted.
///
/// The interface-wide list above says what REST can project for *some*
/// session; this says what it can project for *this* one. Both are needed and
/// they are not the same claim: a service that renders a terminal still cannot
/// render one for a session that has none, and a client told otherwise
/// discovers it by getting an error where it expected a screen.
fn conformance_for(capabilities: &SessionCapabilities) -> Vec<String> {
    // `core` unconditionally: every session has a lifecycle, a log and a
    // status, whatever plane it is driven on.
    let mut classes = vec![format!("{CONF_BASE}/core")];
    for (held, class) in [
        (capabilities.terminal, "terminal"),
        (capabilities.conversation, "conversation"),
        (capabilities.steer, "steering"),
        (capabilities.interrupt, "interrupt"),
        (capabilities.usage, "usage"),
    ] {
        if held {
            classes.push(format!("{CONF_BASE}/{class}"));
        }
    }
    classes
}

/// The interface profile.
pub struct RestInterface;

impl InterfaceProfile for RestInterface {
    fn interface_id(&self) -> &'static str {
        "rest"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::ResolveApproval,
            Operation::Interrupt,
            Operation::Steer,
            Operation::ResizeTerminal,
            Operation::ScreenSnapshot,
            Operation::GetUsage,
        ]
    }
}

/// Request body for creating a session.
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    /// Initial terminal width.
    #[serde(default)]
    pub cols: Option<u16>,
    /// Initial terminal height.
    #[serde(default)]
    pub rows: Option<u16>,
    /// The environment profile to start the session from.
    ///
    /// The whole description of the session: which agent, which model, how
    /// hard it thinks, how often it asks, whose access it spends, and what it
    /// carries. Naming nothing is a choice too — it starts the session the way
    /// this deployment says an unqualified session should start, which is not
    /// the same as a bare one.
    #[serde(default)]
    pub environment_profile: Option<String>,
}

/// A session as this interface renders it.
#[derive(Debug, Serialize)]
pub struct SessionView {
    id: String,
    status: layer2_core::SessionStatus,
    recovery_state: layer2_core::RecoveryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_reason: Option<layer2_core::StopReason>,
    kernel: String,
    cols: u16,
    rows: u16,
    last_seq: u64,
    created_at: String,
    /// Which runtime version is serving this session, and what it has been
    /// through to get there.
    ///
    /// Present only once a session has been moved, because a session that has
    /// never been moved has nothing to explain — and a rotation that goes
    /// unmentioned is a gap in someone's terminal with no account of itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_version: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    rotations: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    rotated_at: Option<String>,
    /// Who started this session, as they should be shown.
    ///
    /// Absent when the caller may not be told — naming somebody is its own
    /// grant — and absent when the provider released nothing worth drawing.
    /// A client renders what it is given and never falls back to the subject
    /// identifier, which is not a name and reads as a fault when shown as one.
    #[serde(skip_serializing_if = "Option::is_none")]
    creator: Option<PublicIdentity>,
    /// What this particular session can be asked to do.
    ///
    /// Derived from the set it was granted at creation rather than from what
    /// this interface can project in general, so a client reads the truth about
    /// the session in front of it instead of the truth about the service.
    conforms_to: Vec<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl From<Session> for SessionView {
    fn from(s: Session) -> Self {
        Self {
            id: s.id.0,
            status: s.status,
            recovery_state: s.recovery_state,
            stop_reason: s.stop_reason,
            kernel: s.kernel.0,
            cols: s.geometry.columns(),
            rows: s.geometry.rows(),
            last_seq: s.last_seq,
            runtime_version: s.runtime_version,
            rotations: s.rotations,
            rotated_at: s.rotated_at,
            created_at: s.created_at,
            creator: None,
            conforms_to: conformance_for(&s.capabilities),
        }
    }
}

/// Attach the owner's identity to each view, as far as this caller may know it.
///
/// One lookup for the whole page rather than one per row: the directory is a
/// database, and a list of twenty sessions is not a reason to ask it twenty
/// times. A directory that cannot be reached costs the names and not the
/// sessions — a list nobody can read is worse than one drawn without faces.
async fn named(users: &UserDirectory, caller: &Caller, sessions: Vec<Session>) -> Vec<SessionView> {
    let owners: Vec<_> = sessions.iter().map(|s| s.owner.clone()).collect();
    let identities = users
        .identities_known_to(caller, &owners)
        .await
        .unwrap_or_default();
    sessions
        .into_iter()
        .map(|s| {
            let creator = identities.get(&s.owner).cloned();
            let mut view = SessionView::from(s);
            view.creator = creator;
            view
        })
        .collect()
}

/// A core failure rendered as an RFC 9457 problem document.
pub(crate) struct Problem(pub(crate) CoreError);

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        // A caller who may not see a session is told it does not exist:
        // confirming the id would itself leak information. The substitution
        // happens once, here, and everything below renders the condition the
        // caller is being told about rather than the one that occurred —
        // matching the status while the body still says "forbidden" answers
        // the very question the refusal exists to withhold.
        let rendered = match &self.0 {
            CoreError::Forbidden => &CoreError::SessionNotFound,
            other => other,
        };
        let (status, slug) = match rendered {
            CoreError::SessionNotFound | CoreError::Forbidden => {
                (StatusCode::NOT_FOUND, "session-not-found")
            }
            CoreError::EnvironmentProfileNotFound => {
                (StatusCode::NOT_FOUND, "environment-profile-not-found")
            }
            CoreError::InvalidRequest(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-request"),
            CoreError::SessionTerminated => (StatusCode::CONFLICT, "session-terminated"),
            CoreError::AlreadyAttached => (StatusCode::CONFLICT, "already-attached"),
            CoreError::InvalidGeometry(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-geometry"),
            CoreError::RuntimeUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "runtime-unavailable")
            }
            CoreError::InvalidState => (StatusCode::CONFLICT, "invalid-state"),
            // 422 rather than 404: the route exists and the request is
            // well-formed, and this session simply cannot honour it. A 404
            // would send the caller looking for a spelling mistake.
            CoreError::CapabilityUnsupported(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "capability-unsupported")
            }
        };
        (
            status,
            Json(serde_json::json!({
                "type": format!("{PROBLEM_BASE}/{slug}"),
                "title": slug.replace('-', " "),
                "status": status.as_u16(),
                "detail": rendered.to_string(),
                "code": rendered.code(),
            })),
        )
            .into_response()
    }
}

impl From<CoreError> for Problem {
    fn from(e: CoreError) -> Self {
        Self(e)
    }
}

/// Environment bundles a session can be started with.
///
/// A resource of its own rather than a property of a session, because a bundle
/// outlives every session that used one — so it has its own router, mounted
/// beside this one.
pub mod environments;

/// What somebody running the service can see and do.
///
/// Its own router under its own path segment, mounted beside this one, so the
/// whole surface can be restricted in front of the service rather than one
/// route at a time.
pub mod admin;

/// What the session routes are served from.
///
/// Two services rather than one because a session names the person who started
/// it, and a subject identifier is the wrong thing to put in front of anybody.
/// Composed with `FromRef` so a handler that does not name anyone keeps asking
/// for exactly what it uses.
#[derive(Clone)]
pub struct SessionsState {
    /// Sessions and the runtimes serving them.
    pub sessions: Arc<SessionService>,
    /// Who has used the service, for naming the owner of one.
    pub users: Arc<UserDirectory>,
}

impl FromRef<SessionsState> for Arc<SessionService> {
    fn from_ref(state: &SessionsState) -> Self {
        Arc::clone(&state.sessions)
    }
}

impl FromRef<SessionsState> for Arc<UserDirectory> {
    fn from_ref(state: &SessionsState) -> Self {
        Arc::clone(&state.users)
    }
}

/// Routes this interface contributes.
pub fn router(state: SessionsState) -> Router {
    Router::new()
        .route("/api/v1/sessions", post(create).get(list))
        .route("/api/v1/sessions/{id}", get(read))
        // Two distinct verbs: ending a session leaves it listed, removing it
        // does not. Collapsing them would make "stop this" and "forget this"
        // the same button.
        .route("/api/v1/sessions/{id}", delete(remove))
        .route("/api/v1/sessions/{id}/terminate", post(terminate))
        // Stopping is not ending. A stopped session keeps its history and can
        // be picked up again; the pair only makes sense together, which is why
        // reviving lives beside it rather than behind a creation call.
        .route("/api/v1/sessions/{id}/stop", post(stop))
        .route("/api/v1/sessions/{id}/revive", post(revive))
        .route("/api/v1/sessions/{id}/screen", get(screen))
        // What the session has shown, for one that has no runtime to attach
        // to. Reading stopped work is not the same as resuming it.
        .route("/api/v1/sessions/{id}/transcript", get(transcript))
        // Progress for a session whose runtime is still starting, and its
        // history thereafter. Resumable, so a reconnect loses nothing.
        .route("/api/v1/sessions/{id}/events", get(events::stream))
        .route("/api/v1/conformance", get(conformance))
        .route("/api/v1/kernels", get(kernels))
        .with_state(state)
}

async fn create(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    // Optional on purpose: a deployment with no delegation configured still
    // creates sessions, they simply cannot call out as the owner.
    credential: Option<axum::Extension<DelegatedCredential>>,
    Json(body): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionView>), Problem> {
    let geometry = match (body.cols, body.rows) {
        (Some(c), Some(r)) => Some(TerminalGeometry::new(c, r).map_err(CoreError::from)?),
        _ => None,
    };
    let session = service
        .create_session(
            &caller,
            SessionSpec {
                environment_profile: body
                    .environment_profile
                    .map(layer2_core::EnvironmentProfileId),
                // Both replaced by the core from the profile this session
                // names. Sent as defaults rather than left out because the
                // spec is also what a rotation rebuilds a session from, where
                // they are the values the session was started with.
                kernel: KernelId::new("codex"),
                geometry,
                options: Default::default(),
                subject_credential: credential.map(|axum::Extension(c)| c.0),
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(session.into())))
}

async fn list(
    State(service): State<Arc<SessionService>>,
    State(users): State<Arc<UserDirectory>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Json<Vec<SessionView>> {
    let sessions = service.list_sessions(&caller).await;
    Json(named(&users, &caller, sessions).await)
}

async fn read(
    State(service): State<Arc<SessionService>>,
    State(users): State<Arc<UserDirectory>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<SessionView>, Problem> {
    let session = service.get_session(&caller, &SessionId(id)).await?;
    let mut views = named(&users, &caller, vec![session]).await;
    Ok(Json(views.remove(0)))
}

async fn terminate(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    service.terminate_session(&caller, &SessionId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    service.stop_session(&caller, &SessionId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn revive(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<SessionView>, Problem> {
    let session = service.revive_session(&caller, &SessionId(id)).await?;
    Ok(Json(session.into()))
}

/// The session's terminal output as the bytes a terminal would render.
///
/// Served as an octet stream rather than JSON because it is an escape-sequence
/// stream: encoding it as a string would mean deciding how to escape it, and
/// the client only has to write it to a terminal.
async fn transcript(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Response, Problem> {
    let bytes = service.session_transcript(&caller, &SessionId(id)).await?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    )
        .into_response())
}

async fn remove(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    // The stored context is removed by the caller that owns object storage;
    // the core reports what to remove rather than reaching for it itself.
    let _context_ref = service.delete_session(&caller, &SessionId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn screen(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, Problem> {
    let snapshot = service.screen_snapshot(&caller, &SessionId(id)).await?;
    Ok(Json(serde_json::json!({
        "cols": snapshot.geometry.columns(),
        "rows": snapshot.geometry.rows(),
        "lines": snapshot.rows,
    })))
}

async fn conformance() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "conformsTo": CONFORMS_TO }))
}

async fn kernels(State(service): State<Arc<SessionService>>) -> Json<serde_json::Value> {
    let ids: Vec<String> = service
        .available_kernels()
        .into_iter()
        .map(|k| k.0)
        .collect();
    Json(serde_json::json!({ "kernels": ids }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rest_projects_every_core_operation() {
        // REST has no structural constraint, so a missing operation here means
        // the facade has drifted from the core rather than that the protocol
        // cannot express it.
        assert_eq!(RestInterface.operations().len(), 9);
    }

    /// Everything a client can observe about a refusal.
    async fn rendered(e: CoreError) -> (StatusCode, serde_json::Value) {
        let response = Problem(e).into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("problem documents are small");
        (
            status,
            serde_json::from_slice(&bytes).expect("a problem document is json"),
        )
    }

    #[tokio::test]
    async fn unauthorized_is_indistinguishable_from_not_found() {
        // Not merely the same status. A body that still says "forbidden"
        // confirms the identifier exists, which is the one thing this refusal
        // is shaped to withhold — so every field a client can read must match.
        let (forbidden_status, forbidden_body) = rendered(CoreError::Forbidden).await;
        let (missing_status, missing_body) = rendered(CoreError::SessionNotFound).await;

        assert_eq!(forbidden_status, StatusCode::NOT_FOUND);
        assert_eq!(missing_status, StatusCode::NOT_FOUND);
        assert_eq!(
            forbidden_body, missing_body,
            "a refusal that reads differently is an existence oracle"
        );
    }

    #[test]
    fn zero_geometry_is_unprocessable_not_a_server_error() {
        let err = TerminalGeometry::new(0, 24).expect_err("zero width fails");
        let response = Problem(CoreError::InvalidGeometry(err)).into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn an_unreachable_runtime_is_service_unavailable() {
        let response = Problem(CoreError::RuntimeUnavailable("down".into())).into_response();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn a_session_declares_only_the_classes_it_was_granted() {
        // The interface-wide list says what REST can project for some session.
        // This says what it projects for this one, and the two are different
        // claims: a client told a screenless session conforms to the terminal
        // class finds out by asking for a screen and getting an error.
        let terminal = conformance_for(&SessionCapabilities {
            terminal: true,
            snapshot: true,
            ..Default::default()
        });
        assert!(terminal.iter().any(|c| c.ends_with("/conf/core")));
        assert!(terminal.iter().any(|c| c.ends_with("/conf/terminal")));
        assert!(
            !terminal.iter().any(|c| c.ends_with("/conf/conversation")),
            "a terminal session must not claim the conversation class: {terminal:?}"
        );

        let conversation = conformance_for(&SessionCapabilities {
            conversation: true,
            steer: true,
            usage: true,
            ..Default::default()
        });
        assert!(conversation
            .iter()
            .any(|c| c.ends_with("/conf/conversation")));
        assert!(conversation.iter().any(|c| c.ends_with("/conf/steering")));
        assert!(conversation.iter().any(|c| c.ends_with("/conf/usage")));
        assert!(
            !conversation.iter().any(|c| c.ends_with("/conf/terminal")),
            "a conversation session must not claim the terminal class: {conversation:?}"
        );
    }

    #[test]
    fn every_session_declares_the_core_class_whatever_it_can_do() {
        // A session that was granted nothing still has a lifecycle, a log and
        // a status, and a client needs to be told it can read those.
        let none = conformance_for(&SessionCapabilities::default());
        assert_eq!(none.len(), 1);
        assert!(none[0].ends_with("/conf/core"));
    }

    #[test]
    fn a_missing_ability_is_unprocessable_rather_than_missing() {
        // 404 would send the caller looking for a spelling mistake. The route
        // exists, the request is well formed, and this session cannot honour
        // it — which is a different thing and has to read as one.
        let response =
            Problem(CoreError::CapabilityUnsupported("conversation".into())).into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn a_view_carries_geometry_as_plain_integers() {
        let s = Session {
            id: SessionId("sesn_1".into()),
            owner: layer2_core::SubjectId("subject-a".into()),
            kernel: KernelId::new("example-runtime"),
            status: layer2_core::SessionStatus::Ready,
            recovery_state: layer2_core::RecoveryState::Normal,
            stop_reason: None,
            options: layer2_core::AgentOptions::default(),
            environment_profile: None,
            geometry: TerminalGeometry::new(120, 40).expect("valid"),
            last_seq: 3,
            runtime_version: None,
            rotations: 0,
            rotated_at: None,
            capabilities: Default::default(),
            created_at: "2026-08-12T00:00:00Z".into(),
        };
        let view: SessionView = s.into();
        let json = serde_json::to_value(&view).expect("serializes");
        assert_eq!(json["cols"], 120);
        assert_eq!(json["rows"], 40);
        assert!(json.get("stop_reason").is_none(), "absent reason omitted");
    }
}
