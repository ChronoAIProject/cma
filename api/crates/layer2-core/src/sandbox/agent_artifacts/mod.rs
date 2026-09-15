//! Explicitly published immutable Agent artifacts. No paths, URLs or runtime
//! homes can select content; resource grants authorize only this registry.

mod publication;
mod service;
mod types;
pub use publication::ArtifactPublication;
pub use service::AgentArtifactService;
pub use types::*;
