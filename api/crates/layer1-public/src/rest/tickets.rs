//! Exchanging a credential for one a WebSocket upgrade can carry.
//!
//! Here rather than in the terminal interface because this is the surface that
//! *can* carry a credential: the browser WebSocket API cannot set a header on
//! an upgrade, so the exchange has to happen over an ordinary request first.
//! What a ticket is, why one in a URL is acceptable, and what is never recorded
//! are all in `crate::tickets`.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use layer2_core::{AttachmentClaim, CoreError, SessionId, SessionService};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::AuthenticatedCaller;
use crate::rest::{OptionalJson, RequestFailure};
use crate::tickets::TerminalTickets;

/// Request body for an attach ticket.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct TicketRequest {
    /// Whether this attachment should supersede whoever holds one.
    ///
    /// Absent means no, which is the right default for a second tab opened by
    /// accident. Asking for it deliberately is how somebody reclaims a session
    /// from a client of their own that has stopped responding.
    #[serde(default)]
    pub takeover: bool,
}

/// An attach ticket, as this interface renders it.
#[derive(Debug, Serialize, ToSchema)]
pub struct TicketView {
    ticket: String,
    expires_at: String,
}

/// Exchange this request's credential for a ticket the upgrade can carry.
///
/// Authorized exactly as attaching is, and by the same code: the ticket is
/// worth nothing more than the attach it stands in for, so the check that
/// matters is the one `attach_terminal` performs on redemption. This one
/// refuses early, so a caller who cannot attach is told now rather than
/// through a socket that opens and closes.
#[utoipa::path(
    post,
    path = "/api/v1/sessions/{id}/terminal-ticket",
    tag = "terminal",
    params(("id" = String, Path)),
    request_body(content = TicketRequest, content_type = "application/json"),
    responses((status = 200, description = "Single-use terminal attachment ticket", body = TicketView))
)]
pub(super) async fn terminal_ticket(
    State(service): State<Arc<SessionService>>,
    State(tickets): State<Arc<TerminalTickets>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    Path(id): Path<String>,
    OptionalJson(body): OptionalJson<TicketRequest>,
) -> Result<Json<TicketView>, RequestFailure> {
    let id = SessionId(id);
    // Reading the session is what establishes that this caller may have it at
    // all; a subject who cannot see a session is told it does not exist, and
    // gets no ticket for it either.
    let session = service.get_session(&caller, &id).await?;
    if !session.capabilities.terminal {
        return Err(CoreError::CapabilityUnsupported("terminal".into()).into());
    }
    let takeover = body.map(|b| b.takeover).unwrap_or_default();
    let issued = tickets
        .issue(
            &caller,
            &id,
            if takeover {
                AttachmentClaim::Takeover
            } else {
                AttachmentClaim::Exclusive
            },
        )
        .await?;
    Ok(Json(TicketView {
        ticket: issued.value,
        expires_at: issued.expires_at,
    }))
}
