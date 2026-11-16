use super::*;
use crate::resource_access::ResourceAccessService;
use std::sync::Arc;

impl AgentCommandService {
    pub fn new(
        store: Arc<dyn AgentCommandStore>,
        access: Arc<ResourceAccessService>,
        authority: Arc<dyn AgentCommandAdmissionAuthority>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            access,
            authority,
            task_experience: Default::default(),
            discovery_wake: None,
        })
    }

    pub fn with_task_experience(
        self: Arc<Self>,
        policy: Arc<crate::sandbox::task_experience::TaskExperiencePolicy>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            access: self.access.clone(),
            authority: self.authority.clone(),
            task_experience: policy,
            discovery_wake: self.discovery_wake.clone(),
        })
    }

    pub fn with_discovery_wake(
        self: Arc<Self>,
        wake: Option<Arc<dyn CommandDiscoveryWake>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            access: self.access.clone(),
            authority: self.authority.clone(),
            task_experience: self.task_experience.clone(),
            discovery_wake: wake,
        })
    }

    pub fn task_experience_policy(
        &self,
    ) -> Arc<crate::sandbox::task_experience::TaskExperiencePolicy> {
        self.task_experience.clone()
    }
}
