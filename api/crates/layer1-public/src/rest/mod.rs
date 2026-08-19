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
///
/// Unknown fields are refused rather than ignored. A caller who sends
/// `{"kernel": "claude_code"}` — which `GET /api/v1/kernels` gives them every
/// reason to try — was getting a session on a different runtime and a 201
/// saying it had worked. §9: a session refused by name is one somebody can
/// plan around; one accepted and silently altered is not.
///
/// The refusal is blanket rather than aimed at `kernel`, because the same
/// silence swallowed `environmentProfile` — the camelCase spelling this
/// project's own SDK and its AG-UI interface both use — and started a session
/// on the deployment's default profile instead of the named one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionRequest {
    /// Declared only so it can be refused in words a caller can act on.
    ///
    /// Left to `deny_unknown_fields`, this gets serde's generic sentence: it
    /// names the field and lists the ones that were expected, which answers
    /// "what is wrong" and not "then how do I get `claude_code`?". That second
    /// question is the only reason anybody sent it, and §3's bar is a refusal
    /// "in words a caller can act on".
    ///
    /// Every other unknown field keeps serde's message, which is right for
    /// them: a typo has no remedy worth spelling out.
    #[serde(default)]
    kernel: Option<String>,
    /// Initial terminal width.
    #[serde(default)]
    cols: Option<u16>,
    /// Initial terminal height.
    #[serde(default)]
    rows: Option<u16>,
    /// The environment profile to start the session from.
    ///
    /// The whole description of the session: which agent, which model, how
    /// hard it thinks, how often it asks, whose access it spends, and what it
    /// carries. Naming nothing is a choice too — it starts the session the way
    /// this deployment says an unqualified session should start, which is not
    /// the same as a bare one.
    #[serde(default)]
    environment_profile: Option<String>,
}

/// What a create request means, once it has been checked.
#[derive(Debug)]
pub struct CreateSessionParts {
    pub geometry: Option<TerminalGeometry>,
    pub environment_profile: Option<String>,
}

impl CreateSessionRequest {
    /// Read the request, or say why it cannot be honoured.
    ///
    /// The fields are private and this is the only way to them, deliberately.
    /// The first version of this check sat in the handler as a loose `if`, and
    /// deleting it left the entire suite green: the test named after the
    /// behaviour asserted that the field *parsed*, which is the opposite of
    /// refused, so #298 could have reopened in silence.
    ///
    /// Declaring `kernel` is what made that possible — a declared field is no
    /// longer unknown, so `deny_unknown_fields` stops guarding it and this
    /// check becomes the only thing standing there. Putting it on the only
    /// path to the data is what makes it hard to lose: removing it now means
    /// editing a function the tests call directly, and removing the call does
    /// not compile.
    pub fn into_parts(self) -> Result<CreateSessionParts, CoreError> {
        if let Some(named) = self.kernel.as_deref() {
            // Refused rather than honoured. Which runtime a session runs on is
            // a property of the session, and §3 forbids an interface deciding
            // one — a request that could name a kernel would resolve
            // differently depending on which deployment received it.
            tracing::debug!(kernel = %named, "a create request named a kernel");
            return Err(CoreError::InvalidRequest(
                "this interface accepts no 'kernel' field: which runtime a session runs on is \
                 decided by the environment profile it names. Choose or create one whose \
                 agent.kernel is the runtime you want (GET or POST /api/v1/environment-profiles) \
                 and send its id as 'environment_profile'"
                    .to_string(),
            ));
        }
        // Geometry travels as a pair or not at all: half of it is not a size.
        let geometry = match (self.cols, self.rows) {
            (Some(c), Some(r)) => Some(TerminalGeometry::new(c, r).map_err(CoreError::from)?),
            _ => None,
        };
        Ok(CreateSessionParts {
            geometry,
            environment_profile: self.environment_profile,
        })
    }
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
        // The ability a session lacks, as a field rather than only as prose.
        //
        // CLAUDE.md §3 requires a refusal to name what is missing "in words a
        // caller can act on". The name was reaching callers only inside
        // `detail`, as the tail of "capability not supported: conversation",
        // so acting on it meant parsing an English sentence — which every
        // client would do slightly differently and which breaks the moment the
        // wording is improved. The conversation projection already sends this
        // as its own field; this makes the two interfaces agree.
        let mut body = problem_document(status, slug, &rendered.to_string(), rendered.code());
        if let CoreError::CapabilityUnsupported(name) = rendered {
            // Inserted rather than always present and usually null: a field
            // there on every failure invites a caller to branch on its
            // emptiness instead of on the code, and the two disagree the first
            // time a refusal arrives without a name.
            if let Some(object) = body.as_object_mut() {
                object.insert("capability".into(), serde_json::Value::String(name.clone()));
            }
        }
        (status, Json(body)).into_response()
    }
}

/// One RFC 9457 problem document, however the failure arose.
///
/// Shared so a request this interface could not read produces the same shape as
/// one the core refused. A caller meeting two different error formats from one
/// endpoint, depending on which half of their request was wrong, has to write
/// two parsers to find out what happened.
pub(crate) fn problem_document(
    status: StatusCode,
    slug: &str,
    detail: &str,
    code: &str,
) -> serde_json::Value {
    serde_json::json!({
        "type": format!("{PROBLEM_BASE}/{slug}"),
        "title": slug.replace('-', " "),
        "status": status.as_u16(),
        "detail": detail,
        "code": code,
    })
}

/// Why a request did not succeed: the body could not be read, or the operation
/// was refused.
///
/// Both halves of a request can be wrong, and until now only one of them
/// answered in this API's own vocabulary. This lets a handler use `?` on a core
/// failure while still rendering a bad body as a problem document.
pub enum RequestFailure {
    /// The body did not parse, or carried something this interface cannot take.
    Body(BodyProblem),
    /// The body was fine and the core would not do it.
    Refused(Problem),
}

impl From<CoreError> for RequestFailure {
    fn from(e: CoreError) -> Self {
        Self::Refused(Problem(e))
    }
}

impl From<BodyProblem> for RequestFailure {
    fn from(e: BodyProblem) -> Self {
        Self::Body(e)
    }
}

impl IntoResponse for RequestFailure {
    fn into_response(self) -> Response {
        match self {
            Self::Body(e) => e.into_response(),
            Self::Refused(e) => e.into_response(),
        }
    }
}

/// A request body this interface could not read, rendered like every other
/// refusal.
///
/// Axum answers a bad body with its own plain text. Wrapping it keeps this
/// API's shape and — the part worth the code — keeps axum's own status, which
/// carries real information: a syntax error is 400, a wrong `Content-Type` is
/// 415, and a body that parsed but could not be honoured is 422. Collapsing all
/// three into one status would send somebody whose header is wrong to go and
/// stare at their JSON.
///
/// Applied to session creation only, so far. The other routes still answer a
/// bad body in axum's words, and saying so is cheaper than implying an
/// evenness this does not yet have.
///
/// One caveat travels with `body_text()`: `CoreError::InvalidRequest` promises
/// to name what is wrong without quoting a value, and this is a foreign string
/// that cannot promise the same. Serde's unknown-field branch carries the key
/// alone, but its `invalid type` branch echoes what was sent. No field on this
/// body is a credential, so there is nothing to leak today — but a field that
/// could carry one must not be added without revisiting this.
pub struct BodyProblem(pub(crate) axum::extract::rejection::JsonRejection);

impl IntoResponse for BodyProblem {
    fn into_response(self) -> Response {
        use axum::extract::rejection::JsonRejection;
        let status = self.0.status();
        // Slug and code together, so the two cannot drift. `Problem` reads its
        // code from `CoreError::code()`; deriving one from the other by string
        // substitution would be a second source of truth that happens to agree.
        let (slug, code) = match &self.0 {
            // Deliberately the same pair the core uses for a request it
            // understood and would not act on. To a caller this is that: the
            // body was read and could not be honoured.
            JsonRejection::JsonDataError(_) => ("invalid-request", "invalid_request"),
            JsonRejection::JsonSyntaxError(_) => ("malformed-json", "malformed_json"),
            JsonRejection::MissingJsonContentType(_) => {
                ("unsupported-media-type", "unsupported_media_type")
            }
            // Axum's rejection is non-exhaustive, so anything added later
            // reads as a body we could not take rather than as a panic.
            _ => ("unreadable-request-body", "unreadable_request_body"),
        };
        // Axum's own sentence. For the unknown-field case it carries serde's
        // message, which names the field and lists what was expected — more
        // than this could say by hand, and it stays correct when a field is
        // added.
        let detail = self.0.body_text();
        let body = problem_document(status, slug, &detail, code);
        (status, Json(body)).into_response()
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
    // Taken as a `Result` so a body this interface cannot honour is rendered
    // as the same problem document as every other refusal. Left to axum, a
    // rejection returns its own plain body, and a caller would meet two
    // different error shapes from one endpoint depending on which half of the
    // request was wrong.
    body: Result<Json<CreateSessionRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<SessionView>), RequestFailure> {
    let Json(body) = body.map_err(BodyProblem)?;
    let parts = body.into_parts()?;
    let session = service
        .create_session(
            &caller,
            SessionSpec {
                environment_profile: parts
                    .environment_profile
                    .map(layer2_core::EnvironmentProfileId),
                geometry: parts.geometry,
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
