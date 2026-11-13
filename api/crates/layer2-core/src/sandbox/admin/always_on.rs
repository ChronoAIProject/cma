//! Administrator approval and revocation of automatic-pause exceptions.

use super::*;
use crate::sandbox::SandboxMaintenanceStore;

impl SandboxOperatorService {
    /// Supply the maintenance persistence port at service composition.
    pub fn with_maintenance(mut self, store: Arc<dyn SandboxMaintenanceStore>) -> Self {
        self.maintenance = Some(store);
        self
    }

    /// Re-approving a retained exception also advances its revision and audits it.
    pub async fn set_always_on(
        &self,
        caller: &Caller,
        id: &SandboxId,
        expected_revision: u64,
        always_on: bool,
    ) -> Result<Sandbox, OperatorError> {
        require(caller, Capability::ControlAnySandbox)?;
        let store = self.maintenance.as_deref().ok_or_else(|| {
            OperatorError::Unavailable("Sandbox maintenance is unavailable".into())
        })?;
        let sandbox = store
            .set_sandbox_always_on(id.clone(), expected_revision, always_on)
            .await
            .map_err(OperatorError::from_sandbox_store)?;
        let mut record = activity(caller, ActivityAction::Configure, &sandbox_target(id));
        record.detail.record(DetailKey::Setting, "always_on");
        record
            .detail
            .record(DetailKey::Status, always_on.to_string());
        record
            .detail
            .record(DetailKey::Revision, sandbox.revision.to_string());
        self.append(record).await?;
        Ok(sandbox)
    }
}
