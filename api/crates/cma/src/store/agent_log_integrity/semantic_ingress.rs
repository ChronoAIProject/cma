//! Owned computation of the original semantic checkpoint and temporary full fold.
use super::*;
use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan,
    IngressClass, IngressOwnershipPolicy, IngressWorkScope,
};
use layer2_core::sandbox::{
    AgentLogKind, AgentTranscriptMessage, AgentTranscriptTurn, QueuedMessageState,
};

type Computation<'a> = AdmittedOperation<'a, Result<SemanticCheckpoint, SandboxStoreError>>;
type Validation<'a> = AdmittedOperation<'a, Result<(), SandboxStoreError>>;

/// Inputs retain caller custody. This computes a checkpoint, including its
/// original untrusted fallback; it grants no audit, quarantine or write authority.
/// The synchronous domain fold has no cancellation point after its first poll.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "enabled recording awaits complete ingress composition"
    )
)]
pub(in crate::store::agent_log_integrity) fn checkpoint_from_entries_admitted<'a>(
    agent: &'a Agent,
    entries: &'a [AgentLogEntry],
    policy: &'a IngressOwnershipPolicy,
) -> Result<Computation<'a>, AdmittedError> {
    let plan = ComputationPlan::inspect(agent, entries)?;
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        plan.working,
        move |working| async move {
            let retained = IngressWorkScope::new(policy.admit(IngressClass::Data, plan.retained)?);
            match SemanticCheckpoint::from_entries(agent, entries) {
                Ok(checkpoint) => Ok(retained.initialize_owned(|| Ok(checkpoint))),
                Err(error) => Ok(working.initialize_owned(|| Err(error))),
            }
        },
    )
}

/// Preserve the strict verdict and recognized unfinished boundaries. The caller
/// retains input custody and any decision to abort, quarantine or write.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "enabled recording awaits complete ingress composition"
    )
)]
pub(in crate::store::agent_log_integrity) fn validate_retained_semantics_admitted<'a>(
    agent: &'a Agent,
    entries: &'a [AgentLogEntry],
    policy: &'a IngressOwnershipPolicy,
) -> Result<Validation<'a>, AdmittedError> {
    let plan = ComputationPlan::inspect(agent, entries)?.validation()?;
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        plan,
        move |working| async move {
            Ok(working.initialize_owned(|| {
                SemanticCheckpoint::validate_retained_semantics(agent, entries)
            }))
        },
    )
}

struct ComputationPlan {
    working: AllocationPlan,
    retained: AllocationPlan,
}

#[derive(Default)]
struct Bytes(usize);

impl Bytes {
    fn copies(&mut self, bytes: usize, count: usize) -> Result<(), AdmissionRefusal> {
        self.0 = bytes
            .checked_mul(count)
            .and_then(|bytes| self.0.checked_add(bytes))
            .filter(|bytes| *bytes <= u32::MAX as usize)
            .ok_or(AdmissionRefusal::InvalidPlan {
                phase: "semantic computation layout",
            })?;
        Ok(())
    }

    fn plan(self, phase: &'static str) -> Result<AllocationPlan, AdmissionRefusal> {
        AllocationPlan::checked(phase, &[self.0])
    }
}

// Rust 1.96.1 nodes have eleven pairs, twelve child pointers and a header.
// Charging two oversized internal nodes per pair also covers split overlap.
fn map_pair_bytes<K, V>() -> usize {
    32 * (size_of::<K>() + size_of::<V>() + 3 * size_of::<usize>())
}

impl ComputationPlan {
    fn validation(self) -> Result<AllocationPlan, AdmissionRefusal> {
        // The reconstructed Agent queue is temporary during strict comparison.
        // Its storage still overlaps the observed checkpoint's retained queue.
        AllocationPlan::checked(
            "retained semantic validation",
            &[
                self.working.requested_bytes(),
                self.retained.requested_bytes(),
            ],
        )
    }

    fn inspect(agent: &Agent, entries: &[AgentLogEntry]) -> Result<Self, AdmissionRefusal> {
        let mut working = Bytes::default();
        let mut retained = Bytes::default();
        // The private queue record is u64, String, bool. Alignment padding makes
        // this a conservative stride without reaching into the domain's fields.
        let queue_stride = size_of::<(u64, String, bool)>() + align_of::<(u64, String, bool)>();
        working.copies(1024 + shared_owner_bytes::<SandboxStoreError>(), 1)?;
        retained.copies(
            shared_owner_bytes::<Result<SemanticCheckpoint, SandboxStoreError>>(),
            1,
        )?;
        let queued = agent
            .conversation
            .queued_messages
            .iter()
            .filter(|message| message.state == QueuedMessageState::Queued)
            .count();
        // Four slots per possible element cover small Vec capacity and old/new
        // growth overlap. Each digest retains 64 bytes; formatting scratch is
        // included in the working allowance before the replacement is built.
        retained.copies(4 * queue_stride + 64, queued)?;
        working.copies(128, queued)?;
        if entries.is_empty() {
            return Ok(Self {
                working: working.plan("empty semantic computation")?,
                retained: retained.plan("retained semantic checkpoint")?,
            });
        }

        working.copies(agent.id.as_str().len(), 6)?;
        working.copies(map_pair_bytes::<u64, &str>(), queued)?;
        working.copies(4 * size_of::<AgentTranscriptTurn>(), entries.len())?;
        // A queued origin can add a message without a corresponding message row.
        working.copies(
            8 * (size_of::<AgentTranscriptMessage>() + size_of::<String>()),
            entries.len(),
        )?;
        let mut max_owner = 0;
        let mut max_owner_timestamp = 0;
        let mut max_conversation = 0;
        let mut max_timestamp = 0;
        for entry in entries {
            max_timestamp = max_timestamp.max(entry.appended_at.len());
            working.copies(entry.appended_at.len(), 4)?;
            match &entry.kind {
                AgentLogKind::OwnerMessage { content } => {
                    max_owner = max_owner.max(content.len());
                    max_owner_timestamp = max_owner_timestamp.max(entry.appended_at.len());
                    working.copies(content.len(), 2)?;
                    working.copies(4 * queue_stride + 128, 1)?;
                    working.copies(map_pair_bytes::<u64, (AgentTranscriptMessage, bool)>(), 1)?;
                }
                AgentLogKind::AgentMessage { content } => working.copies(content.len(), 2)?,
                AgentLogKind::ConversationOpened {
                    conversation_id, ..
                } => {
                    max_conversation = max_conversation.max(conversation_id.len());
                    working.copies(conversation_id.len(), 2)?;
                }
                AgentLogKind::TurnStarted { origin_key, .. } => {
                    working.copies(origin_key.as_ref().map_or(0, String::len), 1)?;
                    // A completed turn with lost frames retains a formatted
                    // failure; include decimal formatting and replacement.
                    working.copies(512, 1)?;
                }
                AgentLogKind::TurnMetadataSet { metadata, .. } => {
                    working.copies(map_pair_bytes::<String, String>(), metadata.len())?;
                    for (key, value) in metadata {
                        working.copies(key.len(), 1)?;
                        working.copies(value.len(), 1)?;
                    }
                }
                AgentLogKind::TurnFailed { failure, .. } => {
                    working.copies(failure.class.len(), 1)?;
                    working.copies(failure.detail.len(), 1)?;
                }
                _ => {}
            }
        }
        for entry in entries {
            if matches!(
                entry.kind,
                AgentLogKind::TurnStarted {
                    origin_key: Some(_),
                    ..
                }
            ) {
                // The full fold retains its queued map after consumption. Even
                // repeated origins may clone the same message into later turns.
                working.copies(max_owner, 2)?;
                working.copies(max_owner_timestamp, 1)?;
            }
        }
        retained.copies(max_conversation, 1)?;
        retained.copies(max_timestamp, 1)?;
        for attempt in &agent.work.worktree_recoveries {
            let original = &attempt.original_worktree;
            // Recovery validation formats path/branch/operation strings, then
            // constructs a WorktreeRequest and a fixed SHA256 hex digest. Eight
            // copies cover formatted-string growth alongside cloned fields.
            for bytes in [
                original.path.len(),
                original.name.len(),
                original.base_commit.len(),
            ] {
                working.copies(bytes, 8)?;
            }
            working.copies(1024, 1)?;
        }
        Ok(Self {
            working: working.plan("semantic computation and full transcript scratch")?,
            retained: retained.plan("retained semantic checkpoint")?,
        })
    }
}

#[cfg(test)]
#[path = "semantic_ingress_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "semantic_validation_ingress_tests.rs"]
mod validation_tests;
