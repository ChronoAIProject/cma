//! The seam through which the console changes a channel's sources.
//!
//! The console never writes a channel itself: attach and detach go through
//! the same record-keyed management functions as REST, which arrive with
//! `CMAEG_DYNAMIC_SOURCES_ENABLED` (#999). Until then the gateway composes
//! [`PendingSourceChanges`], so the pages say plainly that source changes
//! are not available yet instead of offering controls that cannot work.
/// Whether the console may offer source changes now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAvailability {
    /// Attach and detach are offered.
    Available,
    /// The operator has `CMAEG_DYNAMIC_SOURCES_ENABLED` off.
    Disabled,
    /// This image has no record-keyed source mutations yet.
    NotYetAvailable,
}

pub trait SourceChanges: Send + Sync {
    fn availability(&self) -> SourceAvailability;
}

/// The composition until record-keyed source mutations exist: nothing is
/// offered and nothing is written.
pub struct PendingSourceChanges;

impl SourceChanges for PendingSourceChanges {
    fn availability(&self) -> SourceAvailability {
        SourceAvailability::NotYetAvailable
    }
}
