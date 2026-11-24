use axum::{
    extract::{Request, State},
    http::{header::RETRY_AFTER, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json, Router,
};
use std::sync::Arc;
use tokio::sync::watch;
use tower::Layer as _;

use crate::agent_manager::relay::{Relay, DRAIN};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    Serving,
    Draining,
}

#[derive(Clone)]
pub(crate) struct Admission {
    phase: watch::Sender<Phase>,
    drained: watch::Sender<Option<bool>>,
}

impl Admission {
    pub(crate) fn new() -> Self {
        Self {
            phase: watch::Sender::new(Phase::Starting),
            drained: watch::Sender::new(None),
        }
    }

    pub(crate) fn open(&self) -> bool {
        self.phase.send_if_modified(|phase| {
            if *phase != Phase::Starting {
                return false;
            }
            *phase = Phase::Serving;
            true
        })
    }

    pub(crate) fn close(&self) {
        self.phase.send_if_modified(|phase| {
            let changed = *phase != Phase::Draining;
            *phase = Phase::Draining;
            changed
        });
    }

    pub(crate) fn is_draining(&self) -> bool {
        *self.phase.borrow() == Phase::Draining
    }

    pub(crate) async fn stopping(&self) {
        let mut phase = self.phase.subscribe();
        let _ = phase.wait_for(|phase| *phase == Phase::Draining).await;
    }

    pub(super) async fn serving(&self) -> bool {
        let mut phase = self.phase.subscribe();
        let established = phase.wait_for(|phase| *phase != Phase::Starting).await;
        established.is_ok_and(|phase| *phase == Phase::Serving)
    }

    pub(crate) async fn drain(&self) -> Result<(), String> {
        self.close();
        let mut drained = self.drained.subscribe();
        if drained
            .wait_for(Option::is_some)
            .await
            .is_ok_and(|result| *result == Some(true))
        {
            Ok(())
        } else {
            Err("Agent Manager shutdown did not confirm drain".into())
        }
    }

    pub(super) fn finish_drain(&self, result: &Result<(), String>) {
        self.drained.send_replace(Some(result.is_ok()));
    }

    pub(crate) fn wrap(&self, app: Router, relay: Option<Arc<Relay>>) -> Router {
        // Apply the gate around routing itself, including CORS preflight,
        // fallback routes and relay ingress.
        let gate = middleware::from_fn_with_state((self.clone(), relay), gate);
        Router::new().fallback_service(gate.layer(app))
    }
}

async fn gate(
    State((admission, relay)): State<(Admission, Option<Arc<Relay>>)>,
    mut request: Request,
    next: Next,
) -> Response {
    if relay.is_some() && request.method() == Method::POST && request.uri().path() == DRAIN {
        // The existing control path authenticates the signed loopback request
        // before asking the process owner to close admission.
        request.extensions_mut().insert(admission);
        return next.run(request).await;
    }
    let phase = *admission.phase.borrow();
    if phase == Phase::Serving {
        return next.run(request).await;
    }
    let method = request.method();
    let reason = match phase {
        Phase::Starting => "startup",
        _ => "draining",
    };
    let mut response = match (method, request.uri().path()) {
        (&Method::GET | &Method::HEAD, "/livez") => crate::health::liveness().await.into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(RETRY_AFTER, "1")],
            Json(serde_json::json!({"status":"not-ready", "reason":reason})),
        )
            .into_response(),
    };
    if let Some(relay) = relay.filter(|_| phase == Phase::Draining) {
        relay.mark_draining_file_read(&request, &mut response);
    }
    response
}
