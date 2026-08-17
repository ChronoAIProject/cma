//! AG-UI interface (priority 1) — for frontend clients.
//!
//! A projection of the Layer 2 core session model into AG-UI idiom, and the
//! surface a product frontend builds a chat experience on without touching the
//! terminal.
//!
//! It projects **conversation sessions**. A terminal session carries no
//! conversation capability and is refused here by name, which is the declared
//! subset rule: this interface cannot carry a byte stream, there is no honest
//! mapping from an escape sequence to a message event, and a client that needs
//! the literal terminal has an interface for that.
//!
//! `docs/API_AGUI.md` governs. Where this code and that document disagree, one
//! of them is wrong and it is usually this one.

use axum::{
    routing::{get, post},
    Router,
};

use crate::rest::{Problem, SessionsState};
use crate::{InterfaceProfile, Operation};

mod capabilities;
mod project;
mod run;

/// Routes this interface contributes.
pub fn router(state: SessionsState) -> Router {
    Router::new()
        // One request is one turn: the response is the turn, and it ends when
        // the turn does. Creating a session is projected here too — a thread
        // that does not exist is made — because this protocol has no separate
        // call for it and inventing one would add an operation the core lacks.
        .route("/api/v1/agui/run", post(run::run))
        .route("/api/v1/agui/capabilities", get(capabilities::declare))
        .with_state(state)
}

/// The AG-UI projection of the core session model.
#[derive(Debug, Default)]
pub struct AguiInterface;

impl InterfaceProfile for AguiInterface {
    fn interface_id(&self) -> &'static str {
        "agui"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::StreamEvents,
            Operation::ResolveApproval,
            Operation::Interrupt,
            Operation::Steer,
            Operation::GetUsage,
        ]
    }
}
