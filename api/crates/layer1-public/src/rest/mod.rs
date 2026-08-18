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
    Caller, CoreError, EnvironmentProfileService, PublicIdentity, Session, SessionCapabilities,
    SessionId, SessionService, SessionSpec, TerminalGeometry, UserDirectory,
};
use serde::{Deserialize, Serialize};

mod events;
mod operate;
mod tickets;

use crate::auth::{AuthenticatedCaller, DelegatedCredential};
use crate::tickets::TerminalTickets;
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
            Operation::ChangePlane,
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
    /// What this session's credential may reach.
    ///
    /// The other half of a fact whose first half is already here: capabilities
    /// are what a session can *do*, and this is what it can *touch*. A client
    /// shown one and not the other is describing half a session.
    ///
    /// The session's own answer and never its profile's. A profile can be
    /// edited after a session started from it and the session keeps the scope
    /// it was provisioned with, so reading the profile would give the wrong
    /// answer exactly when somebody has changed one.
    reach: Vec<ReachView>,
}

/// One service a session may reach, as this interface renders it.
///
/// Never a credential, never a token, and never an expiry. What a session may
/// reach is a fact about its configuration; what it holds is not.
#[derive(Debug, Clone, Serialize)]
pub struct ReachView {
    /// The identifier the profile stored. Always present.
    id: String,
    /// What the owner recognises it by, when the catalogue offers a name.
    ///
    /// A raw service identifier is not something anybody recognises, and a
    /// catalogue that could not be asked costs the names rather than the
    /// answer: the identifiers alone are still true.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Whether the platform mandates it, so a client can draw it as fixed
    /// rather than guessing which entries cannot be removed.
    mandated: bool,
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
            // Filled by `reach`, which needs the catalogue and the baseline.
            // Empty here rather than absent: a view built without them reports
            // nothing rather than claiming a session reaches nothing.
            reach: Vec::new(),
        }
    }
}

/// Describe what each session may reach, as far as this caller can be told.
///
/// One catalogue lookup for the whole page rather than one per row, for the
/// same reason the directory is asked once: a list of twenty sessions is not a
/// reason to ask NyxID twenty times.
///
/// A catalogue that cannot be reached costs the names and not the answer. §12
/// of the API contract and #132's whole argument turn on that distinction:
/// "we cannot ask right now" is not "you may reach nothing", and returning an
/// error here would report a transient outage as a session with no reach.
async fn reached(
    environments: &EnvironmentProfileService,
    caller: &Caller,
    credential: Option<&str>,
    sessions: &[Session],
    views: &mut [SessionView],
) {
    let mandated: std::collections::HashSet<String> = environments
        .baseline()
        .services()
        .iter()
        .map(|s| s.0.clone())
        .collect();

    let named: std::collections::HashMap<String, layer2_core::CatalogService> = environments
        .available_services(caller, credential)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|service| (service.id.0.clone(), service))
        .collect();

    for (session, view) in sessions.iter().zip(views.iter_mut()) {
        view.reach = session
            .granted_services
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|id| {
                let known = named.get(&id.0);
                ReachView {
                    mandated: mandated.contains(&id.0),
                    name: known.map(|s| s.name.clone()),
                    description: known.and_then(|s| s.description.clone()),
                    id: id.0,
                }
            })
            .collect();
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
pub struct Problem(pub(crate) CoreError);

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
            // 422 and never 503: retrying will not grant it. The request is
            // well-formed and this owner has simply not given the service to
            // anything — which is fixed in NyxID, not by trying again.
            CoreError::ServiceNotGranted(_) => {
                (StatusCode::UNPROCESSABLE_ENTITY, "service-not-granted")
            }
            // 503 and never 422: the owner's services are fine and we could
            // not list them. Retrying is the correct thing to do, which is the
            // whole reason this is a different answer from the one above.
            CoreError::ServiceCatalogUnavailable(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service-catalogue-unavailable",
            ),
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
mod environments_shapes;

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
    /// One-time grants for attaching a terminal from a browser.
    ///
    /// Shared with the PTY interface, which is where they are redeemed. They
    /// are minted here because this is the surface that can carry a
    /// credential, and redeemed there because that is the surface that cannot.
    pub tickets: Arc<TerminalTickets>,
    /// The catalogue and the baseline, for naming what a session may reach.
    ///
    /// Here because a session reports its reach and a raw service identifier
    /// is not something an owner recognises. The catalogue is asked as the
    /// caller, since which services exist is a fact about their account.
    pub environments: Arc<EnvironmentProfileService>,
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

impl FromRef<SessionsState> for Arc<TerminalTickets> {
    fn from_ref(state: &SessionsState) -> Self {
        Arc::clone(&state.tickets)
    }
}

impl FromRef<SessionsState> for Arc<EnvironmentProfileService> {
    fn from_ref(state: &SessionsState) -> Self {
        Arc::clone(&state.environments)
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
        .route("/api/v1/sessions/{id}/plane", post(plane))
        .route("/api/v1/sessions/{id}/revive", post(revive))
        // Exchanges the credential this route can carry for one the WebSocket
        // upgrade can. See `crate::tickets` for why that exchange exists.
        .route(
            "/api/v1/sessions/{id}/terminal-ticket",
            post(tickets::terminal_ticket),
        )
        // What a caller sends to a session, on whichever plane it holds:
        // keystrokes to a terminal, a prompt turn to a conversation. One route
        // because it is one operation in the core, and the session's granted
        // capabilities decide what it means.
        .route("/api/v1/sessions/{id}/input", post(operate::input))
        // Stop what it is doing without ending it. A signal on the terminal
        // plane, the conversation's own cancel on the other.
        .route("/api/v1/sessions/{id}/interrupt", post(operate::interrupt))
        // Adding to a turn already running, which is not the same as starting
        // one — and is refused by name where a kernel cannot tell them apart.
        .route("/api/v1/sessions/{id}/steer", post(operate::steer))
        .route("/api/v1/sessions/{id}/resize", post(operate::resize))
        // Answering what a runtime is blocked on. Conversation sessions only:
        // a terminal kernel asks on its own screen, and its answer is a
        // keystroke, so inventing an operation here would be a second way to
        // say something this session cannot hear.
        .route(
            "/api/v1/sessions/{id}/approvals/{request_id}",
            post(operate::resolve_approval),
        )
        .route("/api/v1/sessions/{id}/usage", get(operate::usage))
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
                geometry,
                // Replaced by the core from the profile this session names.
                // Sent as a default rather than left out because the spec is
                // also what a rotation rebuilds a session from, where these
                // are the values the session was started with.
                options: Default::default(),
                subject_credential: credential.map(|axum::Extension(c)| c.0),
                // This interface takes no plane, so the profile or the
                // deployment decides — which is what it has always done.
                requested_mode: None,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(session.into())))
}

async fn list(
    State(service): State<Arc<SessionService>>,
    State(users): State<Arc<UserDirectory>>,
    State(environments): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<axum::Extension<DelegatedCredential>>,
) -> Json<Vec<SessionView>> {
    let sessions = service.list_sessions(&caller).await;
    let mut views = named(&users, &caller, sessions.clone()).await;
    let credential = credential.map(|axum::Extension(c)| c.0);
    reached(
        &environments,
        &caller,
        credential.as_deref(),
        &sessions,
        &mut views,
    )
    .await;
    Json(views)
}

async fn read(
    State(service): State<Arc<SessionService>>,
    State(users): State<Arc<UserDirectory>>,
    State(environments): State<Arc<EnvironmentProfileService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    credential: Option<axum::Extension<DelegatedCredential>>,
    Path(id): Path<String>,
) -> Result<Json<SessionView>, Problem> {
    let session = service.get_session(&caller, &SessionId(id)).await?;
    let sessions = vec![session];
    let mut views = named(&users, &caller, sessions.clone()).await;
    let credential = credential.map(|axum::Extension(c)| c.0);
    reached(
        &environments,
        &caller,
        credential.as_deref(),
        &sessions,
        &mut views,
    )
    .await;
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

/// Which plane the caller wants this session driven on.
#[derive(Debug, Deserialize)]
pub struct PlaneRequest {
    /// The plane to move to. Naming the one it is already on changes nothing.
    pub plane: layer2_core::SessionMode,
}

/// Move a session onto the other plane.
///
/// A rotation, and answered as one: the session comes back with its identifier,
/// its history and its place in the caller's list, and the view returned is the
/// session mid-handover — `rotating`, which is the window in which a client
/// must refuse input rather than accept and lose it.
///
/// Every refusal is typed and names what is missing, per §3 of the overview: a
/// kernel that does not serve the plane, or one that cannot carry its
/// conversation across, is refused before anything is torn down.
async fn plane(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    Json(body): Json<PlaneRequest>,
) -> Result<Json<SessionView>, Problem> {
    let session = service
        .change_plane(&caller, &SessionId(id), body.plane)
        .await?;
    Ok(Json(session.into()))
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

/// The runtimes a session can be started on, and what each one can be.
///
/// The planes ride with the identifier so a client can refuse an impossible
/// pairing where the person is still choosing, rather than submitting it and
/// rendering the refusal that comes back. Both come from the same core answer,
/// so an interface cannot offer a combination the core would decline.
async fn kernels(State(service): State<Arc<SessionService>>) -> Json<serde_json::Value> {
    let offerings: Vec<serde_json::Value> = service
        .available_kernels()
        .into_iter()
        .map(|(kernel, modes)| serde_json::json!({ "id": kernel.0, "modes": modes }))
        .collect();
    Json(serde_json::json!({ "kernels": offerings }))
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
