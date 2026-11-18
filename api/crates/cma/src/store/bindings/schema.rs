//! The immutable binding index contract shared by startup and explicit migration.

pub(in crate::store) mod indexes_v1;
pub(in crate::store) use indexes_v1::{indexes, COLLECTIONS, LEGACY_AGENT_INDEX};
