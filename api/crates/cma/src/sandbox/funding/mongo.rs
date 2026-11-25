use super::*;
use layer2_core::sandbox_usage_service::SandboxUsageStore;
use layer2_core::{
    rfc3339_at,
    sandbox::store::{
        PageRequest, SandboxLifecycleCommand, SandboxLifecycleCommandResult,
        SandboxLifecycleCommandStore, SandboxStore, SandboxStoreError,
    },
};

pub(super) struct MongoFundingStateStore(pub Arc<MongoSandboxStore>);
fn error(e: SandboxStoreError) -> FundingError {
    match e {
        SandboxStoreError::Conflict => FundingError::Conflict,
        _ => FundingError::Retryable(e.to_string()),
    }
}
#[async_trait]
impl FundingStateStore for MongoFundingStateStore {
    async fn load(&self, id: &SandboxId) -> Result<Option<SandboxBillingState>, FundingError> {
        self.0.sandbox_billing_state(id).await.map_err(error)
    }
    async fn save(&self, state: &mut SandboxBillingState) -> Result<(), FundingError> {
        self.0
            .save_sandbox_billing_state(state)
            .await
            .map_err(error)
    }
    async fn due(&self, now: u64) -> Result<Vec<SandboxBillingState>, FundingError> {
        self.0
            .due_sandbox_billing_states(now, 100)
            .await
            .map_err(error)
    }
    async fn sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, FundingError> {
        self.0.get_sandbox(id).await.map_err(error)
    }
    async fn checkpoint(&self, id: &SandboxId, now: SystemTime) -> Result<(), FundingError> {
        self.0
            .checkpoint_usage_observed(id.as_str(), &rfc3339_at(now), true)
            .await
            .map_err(error)
    }
    async fn events(&self, id: &SandboxId, after: u64) -> Result<Vec<UsageEvent>, FundingError> {
        let mut cursor = (after > 0).then(|| after.to_string());
        let mut all = Vec::new();
        loop {
            let page = self
                .0
                .usage_events(id, &PageRequest { limit: 200, cursor })
                .await
                .map_err(error)?;
            all.extend(page.items);
            cursor = page.next;
            // Bounded backfill: a history gap is visible, never silently skipped.
            if cursor.is_none() || all.len() >= 10_000 {
                return Ok(all);
            }
        }
    }
}

pub(super) struct RuntimeControl {
    pub store: Arc<MongoSandboxStore>,
    pub adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    pub deployment: String,
}
#[async_trait]
impl FundingControl for RuntimeControl {
    async fn deadline(&self, runtime: FundedRuntime<'_>) -> Result<(), FundingError> {
        let sandbox = runtime.target().sandbox();
        let Some(binding) = &sandbox.runtime else {
            return Ok(());
        };
        let request = layer3_adapter::RuntimeAdoptionRequest::new(
            runtime.target().generation(),
            layer3_adapter::RuntimeHandle::new(&binding.runtime_id),
            crate::sandbox::runtime_mapping::sandbox_spec(sandbox, &self.deployment)
                .map_err(|error| FundingError::Invalid(error.to_string()))?,
        )
        .with_expected_image(&binding.runtime_version);
        self.adapter
            .set_runtime_deadline(&request, runtime.until())
            .await
            .map_err(|e| FundingError::Retryable(e.to_string()))
    }
    async fn pause(
        &self,
        runtime: FundingRuntime<'_>,
        detail: &str,
        now: SystemTime,
    ) -> Result<(), FundingError> {
        let sandbox = runtime.sandbox();
        let result = self
            .store
            .schedule_sandbox_lifecycle(
                SandboxLifecycleCommand::pause(
                    sandbox.id.clone(),
                    sandbox.owner.clone(),
                    rfc3339_at(now),
                    Some(format!(
                        "Prepaid funding unavailable: {}",
                        detail.chars().take(180).collect::<String>()
                    )),
                )
                .at_revision(sandbox.revision),
            )
            .await
            .map_err(error)?;
        if let SandboxLifecycleCommandResult::NotEligible(current) = result {
            // Provisioning/installing use their worker's failure status; an already
            // retiring/pausing runtime keeps its confirmed native shutdown deadline.
            tracing::warn!(sandbox_id=%current.id,status=%current.status.name(), "Sandbox funding pause is not eligible in this phase");
        }
        Ok(())
    }
}
