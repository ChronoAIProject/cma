use super::*;

const DEFAULT_LEASE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_RETRY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct WorkspaceWorkerConfig {
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub holder: String,
    pub deployment_id: String,
    pub lease_duration: Duration,
    pub retry_delay: Duration,
}

impl WorkspaceWorkerConfig {
    pub fn new(holder: impl Into<String>, deployment_id: impl Into<String>) -> Self {
        Self {
            execution: Default::default(),
            holder: holder.into(),
            deployment_id: deployment_id.into(),
            lease_duration: DEFAULT_LEASE,
            retry_delay: DEFAULT_RETRY,
        }
    }

    pub fn with_execution(
        mut self,
        execution: layer2_core::sandbox::ProvisioningExecution,
    ) -> Self {
        self.execution = execution;
        self.lease_duration = self.lease_duration.max(Duration::from_secs(
            layer3_adapter::DEFAULT_TOTAL_TIMEOUT_SECONDS + 60,
        ));
        self
    }
}
