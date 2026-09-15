//! Durable allocation identity and fenced progress. These records never grant
//! permission to provision resources, submit a command, or impersonate an owner.
mod authority;
mod model;
mod profiles;
mod sandbox_admission;
mod store;
mod write;

pub use authority::*;
pub use model::*;
pub use profiles::*;
pub use sandbox_admission::*;
pub use store::*;
pub use write::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AllocationError {
    #[error("allocation authority is unavailable or expired")]
    AuthorityUnavailable,
    #[error("allocation identity or authority does not match")]
    IdentityMismatch,
    #[error("message was not admitted by the partition policy")]
    AdmissionDenied,
    #[error("allocation changed; reload its retained identity")]
    Conflict,
    #[error("allocation capacity is exhausted")]
    Capacity,
    #[error("allocation lease is held or expired")]
    LeaseUnavailable,
    #[error("allocation transition is invalid")]
    InvalidTransition,
    #[error("allocation profiles, trust, or capabilities are invalid")]
    InvalidProfiles,
    #[error("allocation history requires an explicit reset")]
    ResetRequired,
    #[error("allocation storage is unavailable or commit outcome is unknown")]
    Unavailable,
    #[error("stored allocation data is inconsistent")]
    Corrupt,
}

fn hash(value: &impl serde::Serialize) -> Result<String, AllocationError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value).map_err(|_| AllocationError::Corrupt)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests;
