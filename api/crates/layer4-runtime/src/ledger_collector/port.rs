//! Integration seams only. No production implementation or runtime activation.
use super::types::*;
use std::{future::Future, sync::Arc};

/// The real implementation must own C's validated exclusion/epoch and A's
/// operation gate/barrier. This trait is not a constructor for either permit.
pub trait Held: Send + Sync {
    fn binding(&self) -> Binding;
    fn source_revision(&self) -> u64;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciled {
    OriginalReceipt(Box<Receipt>),
    /// A has proved original exact evidence and a safe original-operation retry.
    /// Absence of a row or an elapsed deadline alone never produces this case.
    ResumeOriginal,
}

pub trait CollectorPort: Send + Sync {
    type Hold: Held;

    fn load(&self) -> impl Future<Output = Result<Checkpoint>> + Send;
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
    /// Persist and read back exact bytes before returning. Retrying after a
    /// lost reply/takeover returns the same staged identity, never a new UUID.
    /// The bounded staging slot is independent of the worker claim fence.
    fn retain_evidence(
        &self,
        hold: Arc<Self::Hold>,
        checkpoint: &Checkpoint,
        group: Digest,
    ) -> impl Future<Output = Result<Intent>> + Send;
    fn select(
        &self,
        checkpoint: &Checkpoint,
        intent: Intent,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Load the original bytes and verify every reference before any retry.
    fn verify_retained(&self, intent: &Intent) -> impl Future<Output = Result<()>> + Send;
    fn reconcile(
        &self,
        hold: Arc<Self::Hold>,
        intent: &Intent,
        phase: Phase,
    ) -> impl Future<Output = Result<Reconciled>> + Send;
    /// A owns collect_group and must retain the passed guard inside admitted
    /// worker jobs, including when this future is dropped. It revalidates API
    /// authority and complete source selection before effects. No fresh ID.
    fn collect_group(
        &self,
        hold: Arc<Self::Hold>,
        intent: &Intent,
    ) -> impl Future<Output = Result<()>> + Send;
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
