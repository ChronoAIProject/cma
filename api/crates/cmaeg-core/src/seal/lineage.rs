//! The lineage of a hand-off chain between a channel's consumers.
//!
//! A hand-off's limits (hop count, revisits, the per-root budget) are only as
//! trustworthy as the chain they count, so the chain lives sealed inside the
//! event_ref the gateway issues and is never read back from a provider.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    activity::{is_root_id, MAX_HOPS},
    channel::ReplyChoice,
    ids::{ConsumerId, EventId, RecordId},
    ValidationError,
};

/// A hand-off chain's sealed lineage. Lives only in event_refs; never trusted
/// from providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    /// The chain's [`RootId`].
    pub root: String,
    /// The hand-off's position in the chain, 1..=4.
    pub hop: u8,
    /// The consumers that published each hop, in order: one per hop.
    pub path: Vec<ConsumerId>,
    /// Where the chain's automatic replies go.
    pub reply: ReplyChoice,
}

impl Lineage {
    /// Check the chain's shape; pointers are relative to the lineage.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !(1..=MAX_HOPS).contains(&self.hop) {
            return Err(ValidationError::new("/hop", "must be 1 to 4"));
        }
        if self.path.len() != usize::from(self.hop) {
            return Err(ValidationError::new(
                "/path",
                "must name one publisher per hop",
            ));
        }
        let mut seen = BTreeSet::new();
        if self
            .path
            .iter()
            .any(|publisher| publisher.is_default() || !seen.insert(publisher))
        {
            return Err(ValidationError::new(
                "/path",
                "must name distinct consumers other than default",
            ));
        }
        if !is_root_id(&self.root) {
            return Err(ValidationError::new(
                "/root",
                "must be 32 lowercase hex digits",
            ));
        }
        Ok(())
    }
}

/// A hand-off chain's root: 32 lowercase hex digits of SHA-256 over
/// `cmaeg.root.v1`, the channel record id and the root event id, joined by
/// `\x1f`.
///
/// It is scoped to the record because a custom event's id is the producer's
/// `ce.id`, used verbatim, so the same id can start chains on two channels.
/// It is scoped to the record rather than the channel id string because one
/// record can be reached through several id strings.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootId(String);

/// Bytes of the SHA-256 digest a root id keeps.
const ROOT_DIGEST_BYTES: usize = 16;

impl RootId {
    pub fn derive(record_id: &RecordId, root_event_id: &EventId) -> Self {
        let mut input = Vec::new();
        input.extend_from_slice(b"cmaeg.root.v1\x1f");
        input.extend_from_slice(record_id.as_str().as_bytes());
        input.extend_from_slice(b"\x1f");
        input.extend_from_slice(root_event_id.as_str().as_bytes());
        let digest = ring::digest::digest(&ring::digest::SHA256, &input);
        Self(crate::ids::hex(&digest.as_ref()[..ROOT_DIGEST_BYTES]))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
