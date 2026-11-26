use super::worker::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Accepted {
    subject_hash: String,
    requested_at: String,
    deadline_at: String,
}
impl Accepted {
    pub(super) fn from_parent(parent: &AccountDeletion) -> Self {
        Self {
            subject_hash: parent.subject_hash.clone(),
            requested_at: parent.requested_at.clone(),
            deadline_at: parent.deadline_at.clone(),
        }
    }
    pub(super) fn matches(&self, parent: &AccountDeletion) -> bool {
        self == &Self::from_parent(parent)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CheckpointRow {
    #[serde(rename = "_id")]
    pub(super) id: AccountDeletionId,
    pub(super) schema: u32,
    pub(super) kind: String,
    pub(super) accepted: Accepted,
    pub(super) checkpoint: JournalCheckpoint,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Enrollment,
    Claim,
    Operation,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OperationRow {
    #[serde(rename = "_id")]
    pub(super) id: String,
    pub(super) schema: u32,
    pub(super) kind: Kind,
    pub(super) deletion_id: AccountDeletionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) journal_sequence: Option<u64>,
    pub(super) previous_digest: Option<ErasureDigest>,
    pub(super) request: JournalRequest,
    pub(super) receipt: JournalReceipt,
}
pub(super) fn kind(request: &JournalRequest) -> Kind {
    match request.command() {
        JournalCommand::Enroll => Kind::Enrollment,
        JournalCommand::Claim { .. } => Kind::Claim,
        _ => Kind::Operation,
    }
}
pub(super) fn encode(value: &impl Serialize) -> Result<Document, JournalError> {
    let row = mongodb::bson::serialize_to_document(value).map_err(|_| JournalError::Capacity)?;
    if mongodb::bson::serialize_to_vec(&row)
        .map_err(|_| JournalError::Capacity)?
        .len()
        > MAX_JOURNAL_BYTES
    {
        return Err(JournalError::Capacity);
    }
    Ok(row)
}
pub(super) fn checkpoint(row: Document) -> Result<CheckpointRow, JournalError> {
    if mongodb::bson::serialize_to_vec(&row)
        .map_err(|_| JournalError::Incompatible)?
        .len()
        > MAX_JOURNAL_BYTES
    {
        return Err(JournalError::Incompatible);
    }
    let row: CheckpointRow =
        mongodb::bson::deserialize_from_document(row).map_err(|_| JournalError::Incompatible)?;
    if row.schema != JOURNAL_VERSION
        || row.kind != "checkpoint"
        || row.id != row.checkpoint.binding.deletion_id
        || row.checkpoint.binding.contract.version != JOURNAL_VERSION
        || row.checkpoint.parent_revision > i64::MAX as u64
        || row.checkpoint.binding.sequence > i64::MAX as u64
        || row.checkpoint.binding.inventory_generation > i64::MAX as u64
        || !operation_key(&row.checkpoint.last_operation)
        || row
            .checkpoint
            .latest_plan
            .as_deref()
            .is_some_and(|v| !operation_key(v))
        || row
            .checkpoint
            .last_observation
            .as_deref()
            .is_some_and(|v| !operation_key(v))
    {
        return Err(JournalError::Incompatible);
    }
    Ok(row)
}
pub(super) fn operation(row: Document) -> Result<OperationRow, JournalError> {
    if mongodb::bson::serialize_to_vec(&row)
        .map_err(|_| JournalError::Incompatible)?
        .len()
        > MAX_JOURNAL_BYTES
    {
        return Err(JournalError::Incompatible);
    }
    let row: OperationRow =
        mongodb::bson::deserialize_from_document(row).map_err(|_| JournalError::Incompatible)?;
    row.request
        .validate()
        .map_err(|_| JournalError::Incompatible)?;
    let binding = row.request.binding();
    let expected_sequence = binding.sequence + u64::from(row.kind == Kind::Operation);
    let generation = binding.inventory_generation
        + u64::from(matches!(
            row.request.command(),
            JournalCommand::Prepare { .. }
        ));
    let receipt = &row.receipt;
    if receipt.checkpoint.digest != receipt_digest(row.previous_digest.as_ref(), receipt)
        || receipt.checkpoint.last_operation != row.id
        || receipt.checkpoint.parent_revision == 0
        || receipt.checkpoint.parent_revision > i64::MAX as u64
        || receipt.parent_fence.is_some() != receipt.lease_expires_at.is_some()
        || receipt
            .parent_fence
            .is_some_and(|v| v == 0 || v >= i64::MAX as u64)
        || receipt
            .lease_expires_at
            .as_deref()
            .is_some_and(|v| super::records::date(v).is_err())
        || (row.kind == Kind::Enrollment) != row.previous_digest.is_none()
        || (matches!(
            row.request.command(),
            JournalCommand::Claim { .. } | JournalCommand::Renew { .. }
        ) && receipt.parent_fence.is_none())
        || (matches!(
            row.request.command(),
            JournalCommand::Defer { .. } | JournalCommand::Block { .. }
        ) && receipt.parent_fence.is_some())
        || (matches!(row.request.command(), JournalCommand::Prepare { .. })
            && (receipt.checkpoint.latest_plan.as_deref() != Some(row.id.as_str())
                || receipt.checkpoint.last_observation.is_some()))
        || (matches!(row.request.command(), JournalCommand::Observe { .. })
            && receipt.checkpoint.last_observation.as_deref() != Some(row.id.as_str()))
        || row.schema != JOURNAL_VERSION
        || row.kind != kind(&row.request)
        || row.id != row.request.key()
        || row.id != row.receipt.operation
        || row.deletion_id != binding.deletion_id
        || row.journal_sequence != (row.kind == Kind::Operation).then_some(expected_sequence)
        || row.receipt.checkpoint.binding
            != (JournalBinding {
                sequence: expected_sequence,
                inventory_generation: generation,
                ..binding.clone()
            })
    {
        return Err(JournalError::Incompatible);
    }
    Ok(row)
}
fn operation_key(value: &str) -> bool {
    value.len() == 68
        && value.starts_with("aeo_")
        && value[4..]
            .bytes()
            .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase())
}

pub(super) fn receipt_digest(
    previous: Option<&ErasureDigest>,
    receipt: &JournalReceipt,
) -> ErasureDigest {
    let checkpoint = &receipt.checkpoint;
    let encoded = serde_json::to_vec(&(
        previous,
        &receipt.operation,
        &checkpoint.binding,
        checkpoint.parent_revision,
        &checkpoint.last_operation,
        &checkpoint.latest_plan,
        &checkpoint.last_observation,
        receipt.parent_fence,
        &receipt.lease_expires_at,
    ))
    .expect("bounded receipt values serialize");
    let mut hash = Sha256::new();
    for bytes in [
        b"cma.account-erasure.checkpoint.v1".as_slice(),
        encoded.as_slice(),
    ] {
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
        .try_into()
        .expect("fixed digest encoding")
}
