//! Opaque proof of a complete, fenced staged legacy evidence generation.
//! Only completed staging may construct a plan; capture alone cannot do so.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogProjectionState};
use mongodb::bson::Binary;

use super::{
    capture::{CapturedFormat, CapturedLog},
    corrupt,
    publication::PublicationIdentity,
    writer_admission::Admission,
    writer_gate::slots::Permit,
    Head,
};

pub(in crate::store) struct BootstrapPlan {
    current: Agent,
    head: Head,
    leaves: Binary,
    projection: AgentLogProjectionState,
    operation: PublicationIdentity,
    admission: Admission,
    permit: Permit,
}

impl BootstrapPlan {
    /// Recheck the exact staged source and original operation at final publication.
    /// The permit is consumed under its live server-time fence by persist.
    pub(super) fn publication_parts(
        &self,
        captured: &CapturedLog,
        operation: &PublicationIdentity,
        admission: &Admission,
    ) -> Result<(Head, Permit), SandboxStoreError> {
        self.head.validate()?;
        let mut source_head = captured.active_head().clone();
        source_head.evidence_generation = self.head.evidence_generation.clone();
        if captured.format() != CapturedFormat::Legacy
            || self.current != *captured.agent()
            || self.head != source_head
            || self.leaves != *captured.active_leaf_digests()
            || self.projection != *captured.projection()
            || self.operation.key != operation.key
            || self.operation.fingerprint != operation.fingerprint
            || self.operation.appended_at != operation.appended_at
            || mongodb::bson::serialize_to_document(&self.admission).map_err(|_| corrupt())?
                != mongodb::bson::serialize_to_document(admission).map_err(|_| corrupt())?
        {
            return Err(SandboxStoreError::Conflict);
        }
        admission.permits(&self.current, operation, admission.kind)?;
        Ok((self.head.clone(), self.permit.clone()))
    }
}
