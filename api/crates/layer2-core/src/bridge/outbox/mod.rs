//! Explicit Channel reply calls, historical source correlation and conservative
//! delivery state. CMA supplies the durable store and pinned NyxID-only sender.
mod delivery;
mod machine;
pub mod management;
mod model;
mod session;
mod source;
mod text;
mod tool;

pub use delivery::*;
pub use machine::*;
pub use management::BridgeOutboxService;
pub use model::*;
pub use session::*;
pub use source::*;
pub use tool::*;

#[cfg(test)]
mod tests;

fn hash(value: &impl serde::Serialize) -> Result<String, OutboxError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value).map_err(|_| OutboxError::Corrupt)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    #[error("outbox source does not match its original turn")]
    SourceMismatch,
    #[error("outbox output is incomplete or unsupported")]
    OutputUnavailable,
    #[error("reply tool is available only in an active channel-created session")]
    ToolUnavailable,
    #[error("reply tool request key was used for different content")]
    RequestConflict,
    #[error("reply outbox storage is unavailable or its commit is uncertain")]
    Unavailable,
    #[error("outbox output exceeds supported bounds")]
    Bounds,
    #[error("outbox record is invalid")]
    Corrupt,
    #[error("outbox state or worker claim changed")]
    Conflict,
    #[error("outbox delivery authority is unavailable")]
    AuthorityUnavailable,
    #[error("outbox original delivery authority expired")]
    Expired,
    #[error("outbox delivery is suspended")]
    Suspended,
    #[error("outbox bot is waiting for its next permitted send")]
    RateLimited,
    #[error("outbox delivery requires a retained matching key")]
    RetainedKeyRequired,
    #[error("outbox original route or edit capability is unsupported")]
    CapabilityUnavailable,
    #[error("outbox predecessor has not been confirmed sent")]
    PredecessorPending,
    #[error("outbox delivery outcome is unknown and needs explicit recovery")]
    UnknownOutcome,
}
