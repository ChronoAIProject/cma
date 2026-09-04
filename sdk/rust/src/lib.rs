//! Async Rust client for CMA's temporary public discovery boundary.
//!
//! Resource APIs were removed with the retired domain model in issue #801.
//! They return with the replacement public contract in issue #819.
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

mod client;
mod conformance;
mod error;

/// Executable integration guides shipped with the crate.
pub mod guides {
    #[doc = include_str!("../docs/README.md")]
    pub mod overview {}
    #[doc = include_str!("../docs/quickstart.md")]
    pub mod quickstart {}
    #[doc = include_str!("../docs/authentication.md")]
    pub mod authentication {}
    #[doc = include_str!("../docs/errors.md")]
    pub mod errors {}
    #[doc = include_str!("../docs/reference.md")]
    pub mod reference {}
}

pub use client::{Client, ClientBuilder};
pub use conformance::Conformance;
pub use error::{Error, Result};
