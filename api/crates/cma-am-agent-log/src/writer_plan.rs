//! Pure bounded publication planning. The caller supplies the active source
//! proof and persists this plan with its existing aggregate/lease transaction.
use layer2_core::sandbox::{store::SandboxStoreError, AgentLogEntry, AgentLogKind};
use mongodb::bson::{self, Binary, RawDocumentBuf};

use super::capacity::{ensure_append_batch_size, AppendCapacity};
use super::legacy::log_document;
use super::{corrupt, digest, Head, Seal, MAX_BYTES, MAX_SEAL_BYTES, MAX_SEQUENCE};

const NORMAL_COUNT: i64 = 10_000;
pub const NORMAL_BYTES: i64 = 64 * 1024 * 1024;
pub const STOP_COUNT: i64 = NORMAL_COUNT + 64;
pub const STOP_BYTES: i64 = NORMAL_BYTES + 512 * 1024;
pub const DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

/// Permission is resolved from durable gate/admission evidence before planning.
/// Terminal publications cannot turn a normal permit into cleanup-only rolls.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RolloverPermission {
    Closed,
    Normal,
    LegacyRecovery,
    SegmentedRecovery,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Legacy,
    Segmented,
}

pub struct EncodedBatch {
    rows: Vec<RawDocumentBuf>,
    encoded_bytes: i64,
    agent_id: String,
    incarnation: String,
    expected_seq: i64,
}

impl EncodedBatch {
    /// Encode exactly the documents that will be inserted, including query
    /// fields and IDs. Never estimate JSON text or rewrite retained source BSON.
    pub fn new(
        head: &Head,
        entries: &[AgentLogEntry],
        schema_version: i64,
    ) -> Result<Self, SandboxStoreError> {
        ensure_append_batch_size(entries.len())?;
        let mut rows = Vec::with_capacity(entries.len());
        let mut encoded_bytes = 0_i64;
        for (offset, entry) in entries.iter().enumerate() {
            let seq = head
                .last_seq
                .checked_add(offset as i64 + 1)
                .filter(|seq| *seq <= MAX_SEQUENCE)
                .ok_or(SandboxStoreError::SequenceConflict)?;
            if entry.agent_id.as_str() != head.agent_id
                || i64::try_from(entry.seq).ok() != Some(seq)
                || (seq == 1) != matches!(entry.kind, AgentLogKind::Created { .. })
            {
                return Err(SandboxStoreError::SequenceConflict);
            }
            let bytes = bson::serialize_to_vec(&log_document(entry, schema_version)?)
                .map_err(|_| corrupt())?;
            if bytes.len() > DOCUMENT_BYTES {
                return Err(capacity());
            }
            encoded_bytes = encoded_bytes
                .checked_add(bytes.len() as i64)
                .filter(|size| *size <= MAX_BYTES)
                .ok_or_else(capacity)?;
            rows.push(RawDocumentBuf::from_bytes(bytes).map_err(|_| corrupt())?);
        }
        Ok(Self {
            rows,
            encoded_bytes,
            agent_id: head.agent_id.clone(),
            incarnation: head.incarnation.clone(),
            expected_seq: head.last_seq,
        })
    }

    pub fn rows(&self) -> &[RawDocumentBuf] {
        &self.rows
    }

    pub fn encoded_bytes(&self) -> i64 {
        self.encoded_bytes
    }

    pub fn count(&self) -> i64 {
        self.rows.len() as i64
    }
}

pub struct PublicationPlan {
    pub head: Head,
    pub seal: Option<Seal>,
}

pub fn plan(
    current: &Head,
    leaves: Binary,
    batch: &EncodedBatch,
    capacity_class: AppendCapacity,
    permission: RolloverPermission,
    source: SourceFormat,
    operation_key: &str,
) -> Result<PublicationPlan, SandboxStoreError> {
    current.validate()?;
    if batch.agent_id != current.agent_id
        || batch.incarnation != current.incarnation
        || batch.expected_seq != current.last_seq
    {
        return Err(SandboxStoreError::SequenceConflict);
    }
    if operation_key.is_empty()
        || operation_key.len() > 512
        || leaves.subtype != mongodb::bson::spec::BinarySubtype::Generic
        || i64::try_from(leaves.bytes.len()).ok() != current.active_count.checked_mul(32)
        || (source == SourceFormat::Legacy && current.active_ordinal != 0)
        || (permission == RolloverPermission::LegacyRecovery && source != SourceFormat::Legacy)
        || (permission == RolloverPermission::SegmentedRecovery
            && source != SourceFormat::Segmented)
    {
        return Err(corrupt());
    }
    if source == SourceFormat::Legacy && permission == RolloverPermission::Closed {
        return Err(admission_closed());
    }
    // Reject an impossible whole batch before deciding whether to seal. There
    // is no empty-successor loop and no partial publication of the request.
    if !fits(0, 0, batch, capacity_class) {
        return Err(capacity());
    }
    let full_legacy = source == SourceFormat::Legacy
        && (current.active_count >= NORMAL_COUNT || current.active_bytes >= NORMAL_BYTES);
    let recovery = matches!(
        permission,
        RolloverPermission::LegacyRecovery | RolloverPermission::SegmentedRecovery
    );
    let overflows = !batch.rows.is_empty()
        && !fits(
            current.active_count,
            current.active_bytes,
            batch,
            capacity_class,
        );
    let rollover = full_legacy || recovery || overflows;
    if recovery && (!batch.rows.is_empty() || capacity_class != AppendCapacity::Normal) {
        return Err(corrupt());
    }
    if rollover
        && (current.active_count == 0
            || (source == SourceFormat::Segmented && capacity_class != AppendCapacity::Normal))
    {
        return Err(capacity());
    }
    if rollover && permission == RolloverPermission::Closed {
        return Err(admission_closed());
    }
    let mut head = current.clone();
    let seal = if rollover {
        let seal = seal(current, leaves, source, operation_key)?;
        head.active_ordinal = head.active_ordinal.checked_add(1).ok_or_else(corrupt)?;
        head.active_start_seq = head.last_seq.checked_add(1).ok_or_else(corrupt)?;
        head.active_count = 0;
        head.active_bytes = 0;
        head.previous_seal = Some(seal.seal_digest.clone());
        Some(seal)
    } else {
        None
    };
    head.revision = if source == SourceFormat::Legacy {
        1
    } else {
        head.revision.checked_add(1).ok_or_else(corrupt)?
    };
    head.last_seq = head
        .last_seq
        .checked_add(batch.count())
        .filter(|seq| *seq <= MAX_SEQUENCE)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    head.active_count = head
        .active_count
        .checked_add(batch.count())
        .ok_or_else(corrupt)?;
    head.active_bytes = head
        .active_bytes
        .checked_add(batch.encoded_bytes)
        .ok_or_else(corrupt)?;
    head.lifetime_count = head.last_seq;
    head.lifetime_bytes = head
        .lifetime_bytes
        .checked_add(batch.encoded_bytes)
        .ok_or_else(corrupt)?;
    head.projection_through = head.last_seq;
    head.validate()?;
    Ok(PublicationPlan { head, seal })
}

/// Read-only admission hint. Final publication recaptures and validates source.
pub fn needs_normal_rollover(
    head: &Head,
    entries: &[AgentLogEntry],
    schema_version: i64,
) -> Result<bool, SandboxStoreError> {
    head.validate()?;
    let batch = EncodedBatch::new(head, entries, schema_version)?;
    if !fits(0, 0, &batch, AppendCapacity::Normal) {
        return Err(capacity());
    }
    Ok(!entries.is_empty()
        && !fits(
            head.active_count,
            head.active_bytes,
            &batch,
            AppendCapacity::Normal,
        ))
}

fn fits(count: i64, bytes: i64, batch: &EncodedBatch, class: AppendCapacity) -> bool {
    let (max_count, max_bytes) = match class {
        AppendCapacity::Normal => (NORMAL_COUNT, NORMAL_BYTES),
        AppendCapacity::StopOrFailure => (STOP_COUNT, STOP_BYTES),
        AppendCapacity::Delete => (super::MAX_ENTRIES, MAX_BYTES),
    };
    count
        .checked_add(batch.count())
        .is_some_and(|value| value <= max_count)
        && bytes
            .checked_add(batch.encoded_bytes)
            .is_some_and(|value| value <= max_bytes)
}

fn seal(
    head: &Head,
    leaves: Binary,
    source: SourceFormat,
    operation_key: &str,
) -> Result<Seal, SandboxStoreError> {
    let mut value = Seal {
        id: Seal::identity(&head.agent_id, &head.incarnation, head.active_ordinal),
        format_version: 1,
        agent_id: head.agent_id.clone(),
        incarnation: head.incarnation.clone(),
        ordinal: head.active_ordinal,
        start_seq: head.active_start_seq,
        end_seq: head.last_seq,
        entry_count: head.active_count,
        encoded_bytes: head.active_bytes,
        previous_seal: head.previous_seal.clone(),
        prefix_count: head.lifetime_count,
        prefix_bytes: head.lifetime_bytes,
        leaf_digests: leaves,
        seal_digest: digest::binary(Vec::new()),
        digest_version: digest::VERSION.into(),
        sealing_operation: operation_key.into(),
        source_format: match source {
            SourceFormat::Legacy => "legacy_v1",
            SourceFormat::Segmented => "segmented_v1",
        }
        .into(),
        storage_kind: "mongo".into(),
    };
    value.seal_digest = value.digest();
    value.validate()?;
    if bson::serialize_to_vec(&value).map_err(|_| corrupt())?.len() > MAX_SEAL_BYTES {
        return Err(capacity());
    }
    Ok(value)
}

fn capacity() -> SandboxStoreError {
    SandboxStoreError::AgentLogCapacityExceeded
}

fn admission_closed() -> SandboxStoreError {
    // A policy change must never select #1342's deterministic capacity-failure
    // settlement. Keep the original pending publication available for recovery.
    SandboxStoreError::Unavailable(
        "Agent log rollover admission is closed; retain the original operation".into(),
    )
}
