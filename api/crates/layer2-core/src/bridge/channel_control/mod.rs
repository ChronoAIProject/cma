//! Signed exact-source control foundation. No ingress or execution is enabled.
//! Retained intent data never reconstructs a live proof, and an older disabled
//! callback disposition can never become an executable acceptance.

mod intent;
mod parser;
mod proof;
mod reservation;
mod write;

pub use intent::*;
pub use parser::*;
pub use proof::*;
pub use reservation::*;
pub use write::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChannelControlError {
    #[error("invalid signed channel control")]
    Invalid,
    #[error("channel control source unavailable")]
    SourceUnavailable,
    #[error("channel control conflicts with the original intent")]
    Conflict,
    #[error("channel control authority unavailable")]
    AuthorityUnavailable,
    #[error("channel control capability unsupported")]
    CapabilityUnsupported,
    #[error("channel control evidence is corrupt")]
    Corrupt,
    #[error("channel control persistence is unavailable or uncertain")]
    Unavailable,
}

fn hash(value: &impl serde::Serialize) -> Result<String, ChannelControlError> {
    crate::profiles::content_digest(value).map_err(|_| ChannelControlError::Corrupt)
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
