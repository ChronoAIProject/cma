//! Layer 2 — core session model and events.
//!
//! The single tunnel every Layer 1 interface talks to. Protocol-agnostic and
//! kernel-agnostic: no protocol name and no kernel name may appear here, in
//! identifiers, types, comments, or doc examples.
//!
//! The core is a superset, never an intersection: it models the union of what
//! every interface and every runtime needs, so that an interface exposing a
//! subset is a projection rather than a compromise.

pub mod authz;
mod clock;
mod credentials;
pub mod error;
pub mod event;
pub mod ids;
pub mod service;
pub mod session;
pub mod store;
pub mod terminal;

pub use authz::{Authorization, Caller, Capability};
pub use error::CoreError;
pub use event::{Event, EventKind, LiveFrame};
pub use ids::{KernelId, SessionId, SubjectId};
pub use service::{AttachmentHandle, ScreenFactory, SessionService};
pub use session::{RecoveryState, Session, SessionSpec, SessionStatus, StopReason};
pub use store::{ContextStore, SessionRecord, SessionStore, StoreError};
pub use terminal::{InvalidGeometry, ScreenModel, ScreenSnapshot, TerminalGeometry};
