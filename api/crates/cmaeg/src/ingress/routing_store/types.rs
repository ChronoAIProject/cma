use super::codec;
use crate::ingress::routing_plan::ConsumerLaunch;
use cmaeg_core::{
    activity::Activity,
    channel::Source,
    ids::{ChannelId, EventId, RecordId},
};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_SOURCE_BYTES: usize = 256 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const CHUNK_BYTES: usize = 24 * 1024;
pub const MAX_CHUNKS: usize = MAX_PAYLOAD_BYTES.div_ceil(CHUNK_BYTES);
pub const RETENTION_SECONDS: i64 = 7 * 86400;
pub const ROOT_KIND: &str = "ordinary_route_manifest_v1";
pub const PAYLOAD_KIND: &str = "ordinary_route_payload_v1";
pub const NATIVE_INTERACTION_KIND: &str = "native_discord_interaction_payload_v1";
pub const NATIVE_WHATSAPP_INTERACTION_KIND: &str = "native_whatsapp_interaction_payload_v1";
pub const NATIVE_LARK_INTERACTION_KIND: &str = "native_lark_interaction_payload_v1";
pub const MESSAGE_EVENT_KIND: &str = "native_message_event_payload_v1";
pub const CHUNK_KIND: &str = "ordinary_route_chunk_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventIdentity {
    pub owner: String,
    pub channel_id: ChannelId,
    pub record_id: RecordId,
    pub event_id: EventId,
}
impl EventIdentity {
    /// Deliberately excludes request/plan hashes: changed-body reuse conflicts.
    pub fn admission_id(&self) -> String {
        codec::identifier("ordinary-route-root-v1", self)
    }
    pub(super) fn validate(&self) -> Result<(), AdmissionError> {
        if self.owner.is_empty() || self.owner.len() > 512 || self.event_id.as_str().len() > 512 {
            return Err(AdmissionError::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    Invalid,
    TooLarge,
    Conflict,
    Expired,
    /// A conflicting record is no longer readable; never delete/recreate it.
    ExpiredOrUnavailable,
    Unavailable,
    /// A write may have succeeded. The caller retries the same original request.
    Uncertain,
    Corrupt,
    MissingChunk,
}
impl fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // These names contain no request contents or credentials.
        write!(f, "routed admission {self:?}")
    }
}
impl std::error::Error for AdmissionError {}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredTarget {
    /// Stable across retry/restart, including the default target.
    pub id: String,
    pub launch: ConsumerLaunch,
    /// Operator opt-in captured only for a newly admitted ordinary event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_replacement_version: Option<u8>,
}

/// A committed snapshot. Its existence alone never authorizes provider effects.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedPlan {
    pub(super) kind: String,
    pub admission_id: String,
    pub identity: EventIdentity,
    pub request_hash: String,
    pub admitted_at: i64,
    pub retained_until: i64,
    /// Absent on all legacy admissions; never infer a version from raw bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote_projection: Option<u8>,
    /// Reader-first; no legacy admission can acquire replacement authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_preparation_version: Option<u8>,
    pub activity: Activity,
    pub source: Source,
    pub targets: Vec<StoredTarget>,
    /// Exact verified ingress bytes, encrypted at rest. Never a hash-only substitute.
    #[serde(with = "codec::base64_bytes")]
    pub source_body: Vec<u8>,
}
impl fmt::Debug for AdmittedPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdmittedPlan")
            .field("admission_id", &self.admission_id)
            .field("targets", &self.targets.len())
            .field("retained_until", &self.retained_until)
            .finish_non_exhaustive()
    }
}
impl AdmittedPlan {
    pub(crate) fn is_native_interaction(&self) -> bool {
        matches!(
            self.kind.as_str(),
            NATIVE_INTERACTION_KIND
                | NATIVE_WHATSAPP_INTERACTION_KIND
                | NATIVE_LARK_INTERACTION_KIND
                | NATIVE_WHATSAPP_FLOW_INTERACTION_KIND
        )
    }
    pub(crate) fn is_message_event(&self) -> bool {
        self.kind == MESSAGE_EVENT_KIND
    }
    pub(crate) fn prepares_binding(&self, target: &StoredTarget) -> bool {
        self.binding_preparation_version == Some(1)
            && target.launch.provider.slug.as_str() == "cma_codex"
            && target.launch.provider.target.is_profile()
    }
    pub(crate) fn permits_binding_preparation(&self) -> bool {
        self.kind == PAYLOAD_KIND
            && matches!(
                self.activity.kind,
                cmaeg_core::activity::ActivityKind::Message { .. }
            )
            && self.activity.actor.kind == cmaeg_core::activity::ActorKind::Human
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AdmissionState {
    Absent,
    /// Includes a fully uploaded candidate whose root has not committed.
    Partial {
        present: usize,
        total: usize,
    },
    Committed(Box<AdmittedPlan>),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub kind: String,
    pub id: String,
    pub identity: EventIdentity,
    pub request_hash: String,
    pub payload_hash: String,
    pub payload_len: usize,
    pub chunk_hashes: Vec<String>,
    pub admitted_at: i64,
    pub retained_until: i64,
}
impl Manifest {
    pub fn chunk_id(&self, index: usize) -> String {
        codec::identifier(
            "ordinary-route-chunk-v1",
            &(
                &self.identity,
                &self.request_hash,
                &self.payload_hash,
                index,
            ),
        )
    }
    pub fn validate(&self, now: i64) -> Result<(), AdmissionError> {
        self.identity
            .validate()
            .map_err(|_| AdmissionError::Corrupt)?;
        if self.kind != ROOT_KIND
            || self.id != self.identity.admission_id()
            || !codec::is_hash(&self.request_hash)
            || !codec::is_hash(&self.payload_hash)
            || !(1..=MAX_PAYLOAD_BYTES).contains(&self.payload_len)
            || self.chunk_hashes.len() != self.payload_len.div_ceil(CHUNK_BYTES)
            || self.chunk_hashes.len() > MAX_CHUNKS
            || self.chunk_hashes.iter().any(|hash| !codec::is_hash(hash))
            || self.admitted_at <= 0
            || self.retained_until <= self.admitted_at
            || self.retained_until.saturating_sub(self.admitted_at) > RETENTION_SECONDS
        {
            return Err(AdmissionError::Corrupt);
        }
        if self.retained_until <= now {
            return Err(AdmissionError::Expired);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Chunk {
    pub kind: String,
    pub root: String,
    pub request_hash: String,
    pub payload_hash: String,
    pub index: usize,
    pub count: usize,
    pub retained_until: i64,
    #[serde(with = "codec::base64_bytes")]
    pub bytes: Vec<u8>,
}

pub(super) const NATIVE_WHATSAPP_FLOW_INTERACTION_KIND: &str =
    "native_whatsapp_flow_interaction_v1";
