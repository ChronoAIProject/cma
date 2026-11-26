use super::*;

const DEFAULT_LEASE: Duration = Duration::from_secs(15 * 60);
const DEFAULT_RETRY: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct WorkspaceWorkerConfig {
    pub renewable_work: bool,
    pub prepare_observations: bool,
    pub execution: layer2_core::sandbox::ProvisioningExecution,
    pub holder: String,
    pub deployment_id: String,
    pub lease_duration: Duration,
    pub retry_delay: Duration,
}

impl WorkspaceWorkerConfig {
    pub fn new(holder: impl Into<String>, deployment_id: impl Into<String>) -> Self {
        Self {
            renewable_work: false,
            prepare_observations: false,
            execution: Default::default(),
            holder: holder.into(),
            deployment_id: deployment_id.into(),
            lease_duration: DEFAULT_LEASE,
            retry_delay: DEFAULT_RETRY,
        }
    }

    pub fn with_replicas(mut self, enabled: bool) -> Self {
        self.renewable_work = enabled;
        if enabled {
            self.lease_duration =
                Duration::from_secs(layer2_core::sandbox::store::WORK_LEASE_SECONDS);
        }
        self
    }

    pub fn with_prepare_observations(mut self, enabled: bool) -> Self {
        self.prepare_observations = enabled;
        self
    }

    pub fn with_execution(
        mut self,
        execution: layer2_core::sandbox::ProvisioningExecution,
    ) -> Self {
        self.execution = execution;
        if !self.renewable_work {
            self.lease_duration = self.lease_duration.max(Duration::from_secs(
                layer3_adapter::DEFAULT_TOTAL_TIMEOUT_SECONDS + 60,
            ));
        }
        self
    }
}
