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
mod lifecycle;
mod provision;
pub mod reconcile;
mod rotate;

#[cfg(test)]
#[path = "provision_tests.rs"]
mod provision_tests;

#[cfg(test)]
mod test_support;

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod persistence_tests;

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod lifecycle_tests;
pub mod options;
#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
#[cfg(test)]
#[path = "rotate_tests.rs"]
mod rotate_tests;
pub mod service;
pub mod session;
pub mod store;
pub mod terminal;
mod terminal_io;
mod transcript;

pub use authz::{Authorization, Caller, Capability};
pub use error::CoreError;
pub use event::{Event, EventKind, LiveFrame};
pub use ids::{KernelId, SessionId, SubjectId};
pub use options::{AgentOptions, CredentialMode, PermissionMode, ReasoningEffort};
pub use reconcile::Reclaimed;
pub use service::{AttachmentHandle, ScreenFactory, SessionService};
pub use session::{RecoveryState, Session, SessionSpec, SessionStatus, StopReason};
pub use store::{ContextStore, CredentialSource, SessionRecord, SessionStore, StoreError};
pub use terminal::{InvalidGeometry, ScreenModel, ScreenSnapshot, TerminalGeometry};
