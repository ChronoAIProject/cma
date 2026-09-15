//! Platform identity and sender admission only. A normalized key never grants
//! allocation, Agent invocation, management, or outbound delivery authority.
//! Adapters must supply verified capability facts; generic absence is Unknown.

mod admission;
mod key;
mod policy;
mod source;
mod stored;

pub use admission::{PartitionAdmissionDecision, PartitionRefusal};
pub use key::{NormalizedPartitionKey, PartitionContextSharing, ReplyVisibility};
pub(super) use policy::validate_placement;
pub use policy::{ChannelAdmissionPolicy, GroupAdmission, IdentityAdmission};
pub use source::{
    ChannelConversationKind, ChannelEventKind, ChannelSenderKind, LoopEvidence,
    NormalizedPartitionSource, PartitionSourcePin, PlatformCapabilityObservation, StableThreadKind,
    StableThreadReference, ThreadEvidence, VerifiedFlag,
};
pub use stored::{CapabilityContract, StoredPartitionObservation};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PartitionError {
    #[error("partition source evidence does not match its accepted message")]
    SourceMismatch,
    #[error("partition source field is invalid: {0}")]
    InvalidSource(&'static str),
    #[error("partition policy field is invalid: {0}")]
    InvalidPolicy(&'static str),
    #[error("partition key is invalid")]
    InvalidKey,
}

pub(super) fn opaque_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

pub(super) fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests;
