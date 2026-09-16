//! Public bridge setup orchestration with deployment-pinned endpoints and
//! write-only key import. Callback and runtime effect authority stay separate.
mod config;
mod credential;
mod service;

pub use config::*;
pub use credential::*;
pub use service::*;
