//! Async Rust client for CMA public resources and explicit durable work admission.
//! Includes bounded REST polling and pull-based AG-UI, Launch, and Responses streams.
//!
//! ```no_run
//! use cma_sdk::Client;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let client = Client::builder("https://cma.example.com")?.build()?;
//! let deployment = client.read_conformance().await?;
//! println!("{}: {}", deployment.service, deployment.status);
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

#[macro_use]
mod macros;
pub mod admin;
pub mod agent_card;
pub mod agent_events_v2;
pub mod agents;
pub mod agui;
pub mod assets;
pub mod auth;
pub mod bot_setups;
pub mod bots;
mod client;
pub mod cmaeg;
mod conformance;
pub mod defaults;
pub mod discovery;
mod error;
pub mod event_worker_profiles;
pub mod gateway_interactions;
pub mod gateway_records;
mod http;
pub mod instances;
pub mod launches;
pub mod profile_authoring;
pub mod profiles;
pub mod provider_bindings;
pub mod resource_access;
mod resources;
pub mod responses;
pub mod responses_v2;
pub mod stream;
pub mod triggers;

/// Executable integration guides shipped with the crate.
pub mod guides {
    #[doc = include_str!("../docs/README.md")]
    pub mod overview {}
    #[doc = include_str!("../docs/quickstart.md")]
    pub mod quickstart {}
    #[doc = include_str!("../docs/authentication.md")]
    pub mod authentication {}
    #[doc = include_str!("../docs/sandboxes.md")]
    pub mod sandboxes {}
    #[doc = include_str!("../docs/agents.md")]
    pub mod agents {}
    #[doc = include_str!("../docs/profiles.md")]
    pub mod profiles {}
    #[doc = include_str!("../docs/streaming.md")]
    pub mod streaming {}
    #[doc = include_str!("../docs/triggers.md")]
    pub mod triggers {}
    #[doc = include_str!("../docs/operations.md")]
    pub mod operations {}
    #[doc = include_str!("../docs/errors.md")]
    pub mod errors {}
    #[doc = include_str!("../docs/reference.md")]
    pub mod reference {}
}

pub use client::{Client, ClientBuilder};
pub use conformance::Conformance;
pub use error::{Error, Result};
pub use resources::{EventOptions, IdempotencyKey, PageOptions, ResourceList, ResourcePage};
