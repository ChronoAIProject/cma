//! Integration seams only. No production implementation or runtime activation.
use super::types::*;
use std::{future::Future, sync::Arc};

/// The real implementation must own C's validated exclusion/epoch and A's
/// operation gate/barrier. This trait is not a constructor for either permit.
pub trait Held: Send + Sync {
    fn binding(&self) -> Binding;
    fn source_revision(&self) -> u64;
}

pub trait CollectorPort: Send + Sync {
    type Hold: Held;

    fn load(&self) -> impl Future<Output = Result<Checkpoint>> + Send;
    /// Read A's one durable pending slot before new enumeration or rollover,
    /// including with new-work admission closed. Missing/corrupt bytes refuse.
    fn pending_collection(
        &self,
        binding: &Binding,
    ) -> impl Future<Output = Result<Option<CollectionReference>>> + Send;
    /// Read A's original page/evidence, then CAS the original reference into B's
    /// staged progress. Conflicting page, fair position or binding stays held.
    fn recover_reference(
        &self,
        checkpoint: &Checkpoint,
        reference: CollectionReference,
    ) -> impl Future<Output = Result<()>> + Send;
    fn acquire(
        &self,
        checkpoint: &Checkpoint,
    ) -> impl Future<Output = Result<Arc<Self::Hold>>> + Send;
    /// A fresh page resumes strictly after the durable fair group position when
    /// cursor is absent. An opaque cursor is usable only at its pinned revision.
    fn enumerate(
        &self,
        hold: Arc<Self::Hold>,
        checkpoint: &Checkpoint,
    ) -> impl Future<Output = Result<Page>> + Send;
    fn pin_page(
        &self,
        checkpoint: &Checkpoint,
        page: Page,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Observe exactly one group under the gate. For SettledDeleted, call A's
    /// retain_collection and read back exact bytes before returning its reference.
    /// Raw originals live only in A's archive; a lost reply is found by
    /// pending_collection before any fresh request. Retained decisions use no
    /// mutation archive. Each changed-source group gets a fresh operation ID.
    fn observe_and_retain(
        &self,
        hold: Arc<Self::Hold>,
        checkpoint: &Checkpoint,
        group: Digest,
    ) -> impl Future<Output = Result<Staged>> + Send;
    fn verify_prepared(&self, intent: &Intent) -> impl Future<Output = Result<()>> + Send;
    /// CAS the returned immutable reference before Selected; never persist raw
    /// original artifacts in the parent progress document.
    fn record_reference(
        &self,
        checkpoint: &Checkpoint,
        staged: Staged,
    ) -> impl Future<Output = Result<()>> + Send;
    fn select(&self, checkpoint: &Checkpoint) -> impl Future<Output = Result<()>> + Send;
    /// A Archive::read_collection(accepted binding, exact immutable reference).
    fn read_collection(
        &self,
        intent: &Intent,
        reference: &CollectionReference,
    ) -> impl Future<Output = Result<()>> + Send;
    /// A Archive::collection_completion: only the durable final receipt. None
    /// never means success or permission to select a replacement operation.
    fn collection_completion(
        &self,
        intent: &Intent,
        reference: &CollectionReference,
    ) -> impl Future<Output = Result<Option<Receipt>>> + Send;
    /// A's consume/close HELD completion seam, not the missing online collect_group.
    /// Retry reconciles the same original reference. The real adapter must own the
    /// proof, qualified C permit and A source handle, retaining guards across loss
    /// of this future. No production implementation exists in this checkpoint.
    fn complete_collection_held(
        &self,
        hold: Arc<Self::Hold>,
        intent: &Intent,
        reference: &CollectionReference,
    ) -> impl Future<Output = Result<Receipt>> + Send;
    /// Persist the cold receipt before any fair-position or cursor advancement.
    fn record_receipt(
        &self,
        checkpoint: &Checkpoint,
        receipt: Receipt,
    ) -> impl Future<Output = Result<()>> + Send;
    /// CAS the original operation, durable acknowledgement and fair position
    /// together. A changed source revision invalidates the old page/cursor.
    /// A no-effect retained decision may settle without a transition receipt.
    fn settle(&self, checkpoint: &Checkpoint) -> impl Future<Output = Result<()>> + Send;
    fn advance_page(&self, checkpoint: &Checkpoint) -> impl Future<Output = Result<()>> + Send;
    /// Retain the fair position within an unfinished sweep. A completed sweep
    /// starts again at the beginning when new source work arrives. Refuse any
    /// rollover while an operation is pending.
    fn refresh_revision(
        &self,
        checkpoint: &Checkpoint,
        source_revision: u64,
    ) -> impl Future<Output = Result<()>> + Send;
}
