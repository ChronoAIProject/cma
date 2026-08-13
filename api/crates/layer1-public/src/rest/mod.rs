//! REST interface — for backend server clients.
//!
//! The superset facade: it has no structural constraint, so it projects every
//! core operation and serves as the reference realisation. Errors are RFC 9457
//! problem documents, never a bare status code.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use layer2_core::{
    CoreError, KernelId, Session, SessionId, SessionService, SessionSpec, TerminalGeometry,
};
use serde::{Deserialize, Serialize};

mod events;

use crate::auth::{AuthenticatedCaller, DelegatedCredential};
use crate::{InterfaceProfile, Operation};

/// Base URI for problem types this interface returns.
const PROBLEM_BASE: &str = "https://spec.chrono-ai.fun/chrono-bot/1.0/problems";

/// Conformance classes this interface satisfies.
const CONFORMS_TO: &[&str] = &[
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/core",
    "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/terminal",
];

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
    /// Which runtime should back the session.
    pub kernel: String,
    /// Initial terminal width.
    #[serde(default)]
    pub cols: Option<u16>,
    /// Initial terminal height.
    #[serde(default)]
    pub rows: Option<u16>,
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
            created_at: s.created_at,
        }
    }
}

/// A core failure rendered as an RFC 9457 problem document.
pub(crate) struct Problem(CoreError);

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let (status, slug) = match &self.0 {
            // A caller who may not see a session is told it does not exist:
            // confirming the id would itself leak information.
            CoreError::SessionNotFound | CoreError::Forbidden => {
                (StatusCode::NOT_FOUND, "session-not-found")
            }
            CoreError::SessionTerminated => (StatusCode::CONFLICT, "session-terminated"),
            CoreError::AlreadyAttached => (StatusCode::CONFLICT, "already-attached"),
            CoreError::InvalidGeometry(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid-geometry"),
            CoreError::RuntimeUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "runtime-unavailable")
            }
            CoreError::InvalidState => (StatusCode::CONFLICT, "invalid-state"),
        };
        (
            status,
            Json(serde_json::json!({
                "type": format!("{PROBLEM_BASE}/{slug}"),
                "title": slug.replace('-', " "),
                "status": status.as_u16(),
                "detail": self.0.to_string(),
                "code": self.0.code(),
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

/// Routes this interface contributes.
pub fn router(service: Arc<SessionService>) -> Router {
    Router::new()
        .route("/api/v1/sessions", post(create).get(list))
        .route("/api/v1/sessions/{id}", get(read))
        // Two distinct verbs: ending a session leaves it listed, removing it
        // does not. Collapsing them would make "stop this" and "forget this"
        // the same button.
        .route("/api/v1/sessions/{id}", delete(remove))
        .route("/api/v1/sessions/{id}/terminate", post(terminate))
        .route("/api/v1/sessions/{id}/screen", get(screen))
        // Progress for a session whose runtime is still starting, and its
        // history thereafter. Resumable, so a reconnect loses nothing.
        .route("/api/v1/sessions/{id}/events", get(events::stream))
        .route("/api/v1/conformance", get(conformance))
        .route("/api/v1/kernels", get(kernels))
        .with_state(service)
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
                kernel: KernelId::new(body.kernel),
                geometry,
                subject_credential: credential.map(|axum::Extension(c)| c.0),
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(session.into())))
}

async fn list(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
) -> Json<Vec<SessionView>> {
    let sessions = service.list_sessions(&caller).await;
    Json(sessions.into_iter().map(SessionView::from).collect())
}

async fn read(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<Json<SessionView>, Problem> {
    let session = service.get_session(&caller, &SessionId(id)).await?;
    Ok(Json(session.into()))
}

async fn terminate(
    State(service): State<Arc<SessionService>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
) -> Result<StatusCode, Problem> {
    service.terminate_session(&caller, &SessionId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
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

    #[test]
    fn unauthorized_reads_as_not_found() {
        let forbidden = Problem(CoreError::Forbidden).into_response();
        let missing = Problem(CoreError::SessionNotFound).into_response();
        assert_eq!(forbidden.status(), StatusCode::NOT_FOUND);
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
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
    fn a_view_carries_geometry_as_plain_integers() {
        let s = Session {
            id: SessionId("sesn_1".into()),
            owner: layer2_core::SubjectId("subject-a".into()),
            kernel: KernelId::new("example-runtime"),
            status: layer2_core::SessionStatus::Ready,
            recovery_state: layer2_core::RecoveryState::Normal,
            stop_reason: None,
            geometry: TerminalGeometry::new(120, 40).expect("valid"),
            last_seq: 3,
            created_at: "2026-08-12T00:00:00Z".into(),
        };
        let view: SessionView = s.into();
        let json = serde_json::to_value(&view).expect("serializes");
        assert_eq!(json["cols"], 120);
        assert_eq!(json["rows"], 40);
        assert!(json.get("stop_reason").is_none(), "absent reason omitted");
    }
}
