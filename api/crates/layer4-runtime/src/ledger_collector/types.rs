//! Private coordinator views, not new persisted or wire formats. Adapters must
//! validate the frozen A/B/C contracts and exact artifact bytes before projection.
pub type Digest = [u8; 32];
pub type Operation = [u8; 16];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    V1,
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub sha256: Digest,
    pub epoch: u64,
    pub format: SourceFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    InvalidEvidence,
    StaleAuthority,
    StaleCursor,
    Conflict,
    UnknownOutcome,
    ArchiveUnavailable,
    ArchiveCapacity,
}
pub type Result<T> = std::result::Result<T, Refusal>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    SettledDeleted,
    RetainLive,
    RetainUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Prepared,
    SourceCommitted,
    Acknowledged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    pub operation: Operation,
    pub binding: Binding,
    pub group: Digest,
    pub before_revision: u64,
    pub plan_sha256: Digest,
    pub source_sha256: Digest,
    pub observation_sha256: Digest,
    /// Immutable readback reference to exact page/request/observation bytes.
    pub evidence_sha256: Digest,
    pub decision: Decision,
}

impl Intent {
    pub fn valid(&self) -> bool {
        self.operation != [0; 16]
            && self.binding.epoch > 0
            && self.before_revision > 0
            && [
                self.binding.sha256,
                self.group,
                self.plan_sha256,
                self.source_sha256,
                self.observation_sha256,
                self.evidence_sha256,
            ]
            .iter()
            .all(nonzero)
    }
}

/// A owns the actual receipt and its encoding. This view omits source contents
/// and cannot be constructed by deserializing an unverified external response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub intent: Intent,
    pub phase: Phase,
    pub after_revision: u64,
    pub sha256: Digest,
}

impl Receipt {
    pub fn matches(&self, intent: &Intent, phase: Phase) -> bool {
        self.intent == *intent
            && self.phase == phase
            && nonzero(&self.sha256)
            && self.after_revision >= intent.before_revision
            && (phase == Phase::Prepared || self.after_revision > intent.before_revision)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub intent: Intent,
    /// At most three validated original phase receipts, loaded from A's durable
    /// archive. This is a bounded view, not another hot receipt/tombstone store.
    pub receipts: Vec<Receipt>,
}

impl Pending {
    pub fn next_phase(&self) -> Option<Phase> {
        [Phase::Prepared, Phase::SourceCommitted, Phase::Acknowledged]
            .get(self.receipts.len())
            .copied()
    }

    pub fn valid(&self) -> bool {
        self.intent.valid()
            && self.receipts.len() <= 3
            && (self.intent.decision == Decision::SettledDeleted || self.receipts.is_empty())
            && self
                .receipts
                .iter()
                .zip([Phase::Prepared, Phase::SourceCommitted, Phase::Acknowledged])
                .all(|(r, phase)| r.matches(&self.intent, phase))
            && (self.receipts.len() < 3
                || self.receipts[1].after_revision == self.receipts[2].after_revision)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub source_revision: u64,
    pub sha256: Digest,
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub complete: bool,
    pub groups: Vec<Digest>,
    pub encoded_bytes: usize,
}

impl Page {
    pub fn valid(&self) -> bool {
        self.source_revision > 0
            && nonzero(&self.sha256)
            && self.encoded_bytes > 0
            && self.encoded_bytes <= 2 * 1024 * 1024
            && self.groups.len() <= 64
            && self.groups.iter().all(nonzero)
            && self.groups.windows(2).all(|p| p[0] < p[1])
            && self.complete != self.next_cursor.is_some()
            && (self.complete || self.cursor != self.next_cursor)
            && [&self.cursor, &self.next_cursor].iter().all(|c| {
                c.as_ref().is_none_or(|c| {
                    !c.is_empty() && c.len() <= 4096 && !c.chars().any(char::is_control)
                })
            })
    }
}

/// Projection of B's durable progress plus retained original artifacts. The
/// fair-position adapter is still a proposed A/B hook, not deployed persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub revision: u64,
    pub fence: u64,
    pub binding: Binding,
    pub source_revision: u64,
    pub after_group: Option<Digest>,
    pub cursor: Option<String>,
    pub page: Option<Page>,
    pub pending: Option<Pending>,
    pub complete: bool,
}

impl Checkpoint {
    pub fn valid(&self) -> bool {
        self.revision > 0
            && self.fence > 0
            && self.binding.epoch > 0
            && nonzero(&self.binding.sha256)
            && self.source_revision > 0
            && self.after_group.as_ref().is_none_or(nonzero)
            && self.cursor.as_ref().is_none_or(|s| {
                !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control)
            })
            && (!self.complete || (self.page.is_none() && self.pending.is_none()))
            && self.page.as_ref().is_none_or(|p| {
                p.valid() && p.source_revision == self.source_revision && p.cursor == self.cursor
            })
            && self.pending.as_ref().is_none_or(|p| {
                p.valid()
                    && p.intent.binding == self.binding
                    && p.intent.before_revision == self.source_revision
                    && self
                        .page
                        .as_ref()
                        .is_some_and(|page| page.groups.contains(&p.intent.group))
            })
    }
}

fn nonzero(value: &Digest) -> bool {
    *value != [0; 32]
}
