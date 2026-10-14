//! Channel record heads and identifiers, reported to an observer.
//!
//! Management reports every record it writes or deletes, and the resolver
//! every channel id it restores, so the console can keep its owner index and
//! its display-only channel-id map current without reading CMA again. Every
//! call must return at once; see [`crate::turns::observer`].
use cmaeg_core::ids::{ChannelId, RecordId};

use crate::turns::observer::NoopObserver;

/// Receives channel record heads and observed channel ids. Every method
/// defaults to doing nothing.
pub trait ChannelIndexObserver: Send + Sync {
    /// `record` now stores `version`, owned by `owner`.
    fn changed(&self, _record: &RecordId, _owner: &str, _version: u64) {}

    /// `record` was deleted.
    fn removed(&self, _record: &RecordId) {}

    /// A channel id that opens to `record` resolved successfully.
    fn observed(&self, _record: &RecordId, _id: &ChannelId) {}
}

impl ChannelIndexObserver for NoopObserver {}
