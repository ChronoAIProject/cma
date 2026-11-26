//! Admission and receipt reads for a reviewed Sandbox retirement.
use super::*;
use crate::sandbox::reviewed_retirement::{
    ReviewedRetirementMember, ReviewedRetirementOutcome, ReviewedRetirementReceipt,
};

impl SandboxOperatorService {
    pub fn with_reviewed_retirement(
        mut self,
        store: Arc<dyn ReviewedRetirementStore>,
        admit: bool,
    ) -> Self {
        self.reviewed_retirement = Some(ReviewedRetirementAccess { store, admit });
        self
    }

    pub async fn admit_reviewed_retirement(
        &self,
        caller: &Caller,
        id: &SandboxId,
        member: &ReviewedRetirementMember,
    ) -> Result<ReviewedRetirementOutcome, OperatorError> {
        require(caller, Capability::ControlAnySandbox)?;
        let store = self
            .reviewed_retirement
            .as_ref()
            .filter(|access| access.admit)
            .ok_or(OperatorError::NotFound)?;
        let now = now_rfc3339();
        if id != &member.sandbox_id || !member.validate(&now) {
            return Err(OperatorError::InvalidRequest(
                "invalid reviewed retirement member".into(),
            ));
        }
        let mut record = activity(caller, ActivityAction::Delete, &sandbox_target(id));
        record
            .detail
            .record(DetailKey::Status, "reviewed_retirement_requested");
        self.append(record).await?;
        store
            .store
            .admit_reviewed_retirement(member, &now)
            .await
            .map_err(OperatorError::from_sandbox_store)
    }

    pub async fn reviewed_retirement_receipt(
        &self,
        caller: &Caller,
        id: &SandboxId,
        batch: &str,
        member: &str,
    ) -> Result<ReviewedRetirementReceipt, OperatorError> {
        require(caller, Capability::ControlAnySandbox)?;
        let store = self
            .reviewed_retirement
            .as_ref()
            .ok_or(OperatorError::NotFound)?;
        if !crate::sandbox::store::valid_admission_fingerprint(batch)
            || !crate::sandbox::store::valid_admission_fingerprint(member)
        {
            return Err(OperatorError::InvalidRequest(
                "invalid reviewed retirement receipt lookup".into(),
            ));
        }
        store
            .store
            .reviewed_retirement_receipt(id, batch, member)
            .await
            .map_err(OperatorError::from_sandbox_store)?
            .ok_or(OperatorError::NotFound)
    }
}
