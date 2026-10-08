//! Channel summaries: what the console shows about a channel without
//! reading CMA on every page, cached by `(record_id, version)`.
//!
//! A summary is built from the definition `management::load_by_record` reads
//! with REST's owner check, and keeps only names, chips, a status and the
//! detail tabs' settings: never a profile, a credential or a secret. A page
//! render resolves the version from the owner index and reuses the cached
//! summary, so CMA is read only when a channel changed or its summary was
//! evicted.
use std::{collections::BTreeSet, sync::Arc};

use cmaeg_core::{
    channel::{ChannelDefinition, ConsumerView, ReplyChoice, Source, SourceId},
    ids::{ChannelId, ConsumerId, RecordId},
    profile::NyxIdIdentityKind,
};
use moka::sync::Cache;
use serde::Serialize;

use super::{
    detail::{level_label, target_label, ChannelDetail},
    mask,
};

/// CMA's record size limit.
const RECORD_LIMIT: usize = 65_536;
/// Above this share of the limit a summary warns that the record is nearly
/// full.
const NEAR_FULL_PERCENT: usize = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SummaryStatus {
    /// The channel serves events.
    Ready,
    /// Its recoverable creation has not finished.
    Pending,
    /// The definition fails this image's reader rules.
    Invalid,
    /// The record cannot be opened or read.
    Unreadable,
    /// CMA could not be read just now; never cached.
    Unavailable,
}

/// A consumer as the console's chips show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConsumerChip {
    pub id: ConsumerId,
    pub name: String,
    pub label: String,
    /// The provider slug the consumer runs on.
    pub slug: String,
    /// The consumer's public key hash, masked as `sha256:…<last 8>`.
    pub key_hash: String,
    /// Stored before its provider binding exists; routing skips it.
    pub pending: bool,
    /// `profile`, or the existing agent it targets.
    pub target: String,
    pub binding_id: String,
    pub level: &'static str,
    /// Its effective automatic final answer: its own, else the channel's.
    pub auto_final: bool,
    /// Where its automatic outputs go.
    pub destination: ReplyChoice,
    /// Whether a leading `@name` or `/to name` may address it.
    pub mentionable: bool,
    /// Whether it answers non-human actors (custom events and hand-offs).
    pub non_human: bool,
    /// True when `non_human` is the consumer's own setting rather than its
    /// provider's default from `CMAEG_NON_HUMAN_PROVIDERS`.
    pub non_human_set: bool,
    /// Whether it may publish hand-offs, and to which consumers; empty
    /// targets mean any routable consumer.
    pub publish: Option<Vec<ConsumerId>>,
}

impl ConsumerChip {
    /// The chip of `consumer`, its capability read against the configured
    /// non-human provider set.
    fn of(consumer: &ConsumerView<'_>, non_human_providers: &BTreeSet<String>) -> Self {
        Self {
            id: consumer.id.clone(),
            name: mask::display_name(consumer.name),
            label: mask::display_name(consumer.label),
            slug: consumer.provider.slug.to_string(),
            key_hash: mask::hash(consumer.agent_key_hash),
            pending: consumer.pending,
            target: target_label(&consumer.provider.target),
            binding_id: consumer.provider.binding_id.to_string(),
            level: level_label(consumer.provider.level),
            auto_final: consumer.reply.auto_final,
            destination: consumer.reply.destination,
            mentionable: consumer.mentionable,
            non_human: consumer.non_human_effective(non_human_providers),
            non_human_set: consumer.non_human.is_some(),
            publish: consumer
                .publish
                .enabled
                .then(|| consumer.publish.targets.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelSummary {
    /// The definition's owner, compared with the reader's authority on
    /// every read.
    pub owner: String,
    pub owner_kind: Option<NyxIdIdentityKind>,
    pub name: String,
    pub created_at: String,
    /// The channel's source slots, by the same id REST and routing use.
    pub sources: Vec<SourceId>,
    pub consumers: Vec<ConsumerChip>,
    pub routed: bool,
    pub default_receives_all: Option<bool>,
    pub status: SummaryStatus,
    pub record_bytes: u32,
    pub warnings: Vec<&'static str>,
    /// What the detail tabs show; absent when the definition was unreadable.
    pub detail: Option<ChannelDetail>,
}

impl ChannelSummary {
    /// The summary of a definition read for its owner. `non_human_providers`
    /// is `CMAEG_NON_HUMAN_PROVIDERS`, which decides a consumer's actor-kind
    /// capability unless it sets its own.
    pub fn of(
        definition: &ChannelDefinition,
        record_bytes: usize,
        valid: bool,
        non_human_providers: &BTreeSet<String>,
    ) -> Self {
        let status = if !valid {
            SummaryStatus::Invalid
        } else if definition
            .creation
            .as_ref()
            .is_some_and(|value| !value.ready)
        {
            SummaryStatus::Pending
        } else {
            SummaryStatus::Ready
        };
        let mut warnings = Vec::new();
        if record_bytes * 100 > RECORD_LIMIT * NEAR_FULL_PERCENT {
            warnings.push("record_nearly_full");
        }
        Self {
            owner: definition.owner.subject.clone(),
            owner_kind: Some(definition.owner.kind),
            name: definition.name.clone(),
            created_at: definition.created_at.clone(),
            sources: definition.sources.iter().map(Source::source_id).collect(),
            consumers: definition
                .consumers()
                .map(|consumer| ConsumerChip::of(&consumer, non_human_providers))
                .collect(),
            // Legacy channels only until routing policies exist.
            routed: false,
            default_receives_all: None,
            status,
            record_bytes: u32::try_from(record_bytes).unwrap_or(u32::MAX),
            warnings,
            detail: Some(ChannelDetail::of(definition)),
        }
    }

    /// A record whose head names `holder` as its owner but whose definition
    /// cannot be read: shown by status only.
    pub fn without_definition(holder: &str, status: SummaryStatus) -> Self {
        Self {
            owner: holder.to_owned(),
            owner_kind: None,
            name: String::new(),
            created_at: String::new(),
            sources: Vec::new(),
            consumers: Vec::new(),
            routed: false,
            default_receives_all: None,
            status,
            record_bytes: 0,
            warnings: Vec::new(),
            detail: None,
        }
    }
}

/// The channel id a definition's recoverable creation retained, the one
/// channel id a record itself reveals.
pub fn created_channel_id(definition: &ChannelDefinition) -> Option<ChannelId> {
    definition
        .creation
        .as_ref()
        .map(|creation| creation.channel_id.clone())
}

/// Summaries by `(record_id, version)`, least recently used first out.
pub struct Summaries {
    cache: Cache<(RecordId, u64), Arc<ChannelSummary>>,
}

impl Summaries {
    pub fn new(max: u64) -> Self {
        Self {
            cache: Cache::new(max),
        }
    }

    /// The cached summary of `record` at `version`. Callers compare its
    /// owner with the reader's authority.
    pub fn get(&self, record: &RecordId, version: u64) -> Option<Arc<ChannelSummary>> {
        self.cache.get(&(record.clone(), version))
    }

    pub fn insert(&self, record: RecordId, version: u64, summary: Arc<ChannelSummary>) {
        self.cache.insert((record, version), summary);
    }
}
