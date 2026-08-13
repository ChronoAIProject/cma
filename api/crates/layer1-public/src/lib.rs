//! Layer 1 — public interfaces.
//!
//! Five interface types, each a projection of the Layer 2 core session model.
//! An interface may expose a subset of core operations where its protocol
//! cannot express one. It must never invent an operation absent from Layer 2,
//! and must never carry business logic.
//!
//! This crate depends on `layer2-core` and must never depend on
//! `layer3-adapter`.

pub mod a2a;
pub mod agui;
pub mod auth;
pub mod mcp;
pub mod pty;
pub mod rest;

pub use auth::{ActedUpon, AuthenticatedSubject};

/// A semantic operation on the core session model. Each interface declares the
/// subset it projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    /// Create a session.
    CreateSession,
    /// Send input to a session.
    SendInput,
    /// Stream session events.
    StreamEvents,
    /// Resolve a pending approval.
    ResolveApproval,
    /// Interrupt the in-flight turn.
    Interrupt,
    /// Steer an in-flight turn.
    Steer,
    /// Resize the runtime terminal.
    ResizeTerminal,
    /// Fetch a terminal screen snapshot.
    ScreenSnapshot,
    /// Read usage for a session.
    GetUsage,
    /// Author and reuse environment bundles a session can be started with.
    ManageEnvironmentProfiles,
}

/// An interface declares which operations it projects, so callers can discover
/// the surface rather than probing it.
pub trait InterfaceProfile {
    /// Stable identifier for this interface.
    fn interface_id(&self) -> &'static str;

    /// Operations this interface projects.
    fn operations(&self) -> &'static [Operation];
}
