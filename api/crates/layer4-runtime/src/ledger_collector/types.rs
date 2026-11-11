//! Private coordinator views, not new persisted or wire formats. Adapters must
//! validate the frozen A/B/C contracts and exact artifact bytes before projection.
pub type Digest = [u8; 32];
pub type Operation = [u8; 16];
pub use layer3_adapter::LedgerCollectionArtifactReferenceV1 as CollectionReference;

pub fn hex(value: &Digest) -> String {
    value.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    V1,
    V2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub sha256: Digest,
    pub archive_id: Operation,
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
    pub artifact_lengths: [u64; 3],
    pub decision: Decision,
}

impl Intent {
    pub fn valid(&self) -> bool {
        self.reference().validate()
            && self.operation != [0; 16]
            && self.binding.archive_id != [0; 16]
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

impl Intent {
    pub fn reference(&self) -> CollectionReference {
        CollectionReference {
            format: layer3_adapter::LEDGER_COLLECTION_ARTIFACT_FORMAT.into(),
            archive_id: uuid::Uuid::from_bytes(self.binding.archive_id).to_string(),
            archive_version: 3,
            operation_id: uuid::Uuid::from_bytes(self.operation).to_string(),
            group_key: hex(&self.group),
            binding_sha256: hex(&self.binding.sha256),
            plan_sha256: hex(&self.plan_sha256),
            artifacts_sha256: hex(&self.evidence_sha256),
            artifact_lengths: self.artifact_lengths,
        }
    }
}

/// Projection of the frozen v1 receipt fields. The adapter must first validate
/// A's actual encoding, version and receipt checksum. B never synthesizes phases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub version: u16,
    pub binding_sha256: Digest,
    pub operation: Operation,
    pub plan_sha256: Digest,
    pub group: Digest,
    pub source_sha256: Digest,
    pub observation_sha256: Digest,
    pub before_sha256: Digest,
    pub after_sha256: Digest,
    pub before_revision: u64,
    pub after_revision: u64,
    pub operations: u64,
    pub authority: u64,
    pub cleanup: u64,
    pub free_pages_before: u64,
    pub free_pages_after: u64,
    pub phase: Phase,
    pub sha256: Digest,
}

impl Receipt {
    pub fn matches(&self, intent: &Intent) -> bool {
        self.version == 1
            && self.phase == Phase::Acknowledged
            && self.binding_sha256 == intent.binding.sha256
            && self.operation == intent.operation
            && self.plan_sha256 == intent.plan_sha256
            && self.group == intent.group
            && self.source_sha256 == intent.source_sha256
            && self.observation_sha256 == intent.observation_sha256
            && self.before_revision == intent.before_revision
            && self.before_revision.checked_add(1) == Some(self.after_revision)
            && [self.before_sha256, self.after_sha256, self.sha256]
                .iter()
                .all(nonzero)
    }
}

/// B's private reference-before-Selected projection. Raw originals are held
/// only by A's retained archive. Concrete envelope integration uses A's type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub intent: Intent,
    pub reference: Option<CollectionReference>,
}

impl Staged {
    pub fn valid(&self) -> bool {
        self.intent.valid()
            && match self.intent.decision {
                Decision::SettledDeleted => self
                    .reference
                    .as_ref()
                    .is_some_and(|r| r.validate() && *r == self.intent.reference()),
                _ => self.reference.is_none(),
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub intent: Intent,
    pub reference: Option<CollectionReference>,
    /// Only a cold, final receipt is exposed by A's addendum-2 API.
    pub completion: Option<Receipt>,
}

impl Pending {
    pub fn valid(&self) -> bool {
        self.intent.valid()
            && match self.intent.decision {
                Decision::SettledDeleted => {
                    self.reference == Some(self.intent.reference())
                        && self
                            .completion
                            .as_ref()
                            .is_none_or(|r| r.matches(&self.intent))
                }
                _ => self.reference.is_none() && self.completion.is_none(),
            }
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
    pub staged: Option<Staged>,
    pub pending: Option<Pending>,
    pub complete: bool,
}

impl Checkpoint {
    pub fn has_work(&self) -> bool {
        self.staged.is_some() || self.pending.is_some()
    }

    fn contains_first(&self, intent: &Intent) -> bool {
        intent.binding == self.binding
            && intent.before_revision == self.source_revision
            && self
                .page
                .as_ref()
                .is_some_and(|p| p.groups.first() == Some(&intent.group))
    }

    pub fn valid(&self) -> bool {
        self.revision > 0
            && self.fence > 0
            && self.binding.archive_id != [0; 16]
            && self.binding.epoch > 0
            && nonzero(&self.binding.sha256)
            && self.source_revision > 0
            && self.after_group.as_ref().is_none_or(nonzero)
            && self.cursor.as_ref().is_none_or(|s| {
                !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control)
            })
            && (!self.complete
                || (self.page.is_none() && !self.has_work() && self.cursor.is_none()))
            && self.page.as_ref().is_none_or(|p| {
                p.valid()
                    && p.source_revision == self.source_revision
                    && p.cursor == self.cursor
                    && p.groups
                        .first()
                        .is_none_or(|first| self.after_group.is_none_or(|after| *first > after))
            })
            && !(self.staged.is_some() && self.pending.is_some())
            && self
                .staged
                .as_ref()
                .is_none_or(|s| s.valid() && self.contains_first(&s.intent))
            && self
                .pending
                .as_ref()
                .is_none_or(|p| p.valid() && self.contains_first(&p.intent))
    }
}

fn nonzero(value: &Digest) -> bool {
    *value != [0; 32]
}
