//! Async Rust client for the CMA REST interface.
//!
//! The crate is deliberately independent from CMA's service workspace. Its
//! types describe the public wire contract and are pinned to server-owned JSON
//! fixtures, so using the SDK never links a service implementation crate into a
//! consumer.
//!
//! # Quick start
//!
//! ```no_run
//! use cma_sdk::{Client, CreateSessionRequest, EventKind, SessionInput};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::builder("https://cma.example.com")?
//!     .credential_provider(|| std::env::var("CMA_ACCESS_TOKEN"))
//!     .build()?;
//!
//! let session = client
//!     .create_session(CreateSessionRequest::default())
//!     .await?;
//! client
//!     .send_input(&session.id, SessionInput::text("Summarise this repository"))
//!     .await?;
//!
//! let mut events = client.stream_events(&session.id, None).await?;
//! while let Some(event) = events.next().await {
//!     let event = event?;
//!     if matches!(event.kind, EventKind::TurnCompleted) {
//!         break;
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The live PTY byte stream is intentionally absent from this first release.
//! It is CMA's priority-3, CLI-shaped interface; this crate covers the REST
//! terminal operations (resize and screen snapshot) but does not pretend a
//! bidirectional terminal attachment is an HTTP operation.

#![deny(missing_docs)]

mod client;
mod error;
mod event;
mod stream;
mod types;

/// Executable integration guides shipped with the crate.
///
/// Each child module includes one file from `docs/`. Rustdoc therefore tests
/// every Rust fence in the published guides as part of `cargo test --doc`.
pub mod guides {
    #[doc = include_str!("../docs/README.md")]
    pub mod overview {}
    #[doc = include_str!("../docs/quickstart.md")]
    pub mod quickstart {}
    #[doc = include_str!("../docs/authentication.md")]
    pub mod authentication {}
    #[doc = include_str!("../docs/sessions.md")]
    pub mod sessions {}
    #[doc = include_str!("../docs/streaming.md")]
    pub mod streaming {}
    #[doc = include_str!("../docs/operations.md")]
    pub mod operations {}
    #[doc = include_str!("../docs/errors.md")]
    pub mod errors {}
    #[doc = include_str!("../docs/reference.md")]
    pub mod reference {}
}

pub use client::{Client, ClientBuilder, RetryPolicy};
pub use error::{Error, Result};
pub use event::{
    AvailableCommand, ConfigOption, Event, EventKind, PlanStep, ProvisioningStage,
    RuntimeLossCause, SessionPlane, ToolBody,
};
pub use stream::EventStream;
pub use types::{
    Agent, ApprovalDecision, CreateSessionRequest, CredentialMode, MAX_PAGE_SIZE, MIN_PAGE_SIZE,
    Page, PageContinuation, PageWalkLimits, PermissionMode, PublicIdentity, Reach, ReasoningEffort,
    RecoveryState, ScreenSnapshot, Session, SessionId, SessionInput, SessionPageRequest,
    SessionPricing, SessionSize, SessionStatus, Skill, StopReason, TerminalGeometry, Usage,
};
