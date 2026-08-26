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
//! ## A thread is a session, and a new one is named by the service
//!
//! A `threadId` that names nothing creates a session, which is how this
//! protocol projects creation. The session that comes back has an identifier
//! the core minted rather than the one that was asked for — identifiers are
//! the core's to issue, and honouring a caller's would let one be chosen to
//! collide with somebody else's. Every event on the run carries the real one,
//! so a client learns what its thread is called from the first thing it reads.
//!
//! `docs/API_AGUI.md` governs. Where this code and that document disagree, one
//! of them is wrong and it is usually this one.

use std::time::Duration;

use axum::{
    routing::{get, post},
    Router,
};

use crate::rest::{Problem, SessionsState};
use crate::{InterfaceProfile, Operation};

mod admission;
mod capabilities;
#[cfg(test)]
#[path = "contract_tests.rs"]
mod contract_tests;
mod frame;
mod live;
mod project;
mod replay;
mod resources;
mod run;
mod run_ending;
mod state;
mod thread_alias;
mod turn_admission;
#[cfg(test)]
#[path = "steer_tests.rs"]
mod steer_tests;

pub use state::initial_state;

pub(super) const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// Routes this interface contributes.
pub fn router(state: SessionsState) -> Router {
    Router::new()
        // One request is one turn: the response is the turn, and it ends when
        // the turn does. Creating a session is projected here too — a thread
        // that does not exist is made — because this protocol has no separate
        // call for it and inventing one would add an operation the core lacks.
        .route("/api/v1/agui/run", post(run::run))
        // History, and following it, in this protocol's own vocabulary.
        // §7 tells a client to recover a dropped stream by reading the log
        // rather than re-posting the run; until this existed it sent them to
        // a route that answers in the core's vocabulary instead, and a client
        // that followed the instruction could read none of what came back.
        .route("/api/v1/agui/events", get(replay::replay))
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
