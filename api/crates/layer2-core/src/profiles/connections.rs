//! Model access and NyxID integrations have separate connection authority.

use std::collections::BTreeSet;

use crate::ServiceId;

use super::{ModelAccess, SandboxProfileContent};

impl SandboxProfileContent {
    /// Existing published revisions may contain a model service in the old
    /// service picker. Interpret it without rewriting the immutable revision.
    pub fn nyxid_connection_services(&self) -> Vec<ServiceId> {
        let provider = self.agent_provider.connection_services();
        self.nyxid_services
            .iter()
            .filter(|service| !provider.contains(service.as_str()))
            .cloned()
            .collect()
    }

    /// A stable sequence lets the browser finish NyxID integrations first,
    /// then connect the selected model provider before resource assignment.
    pub fn connection_services(&self) -> Vec<ServiceId> {
        let provider = self.agent_provider.connection_services();
        let mut services = self.nyxid_connection_services();
        services.push(ServiceId::new(match self.model_access {
            ModelAccess::ChronoaiManaged => provider.managed,
            ModelAccess::OwnCredentials => provider.subscription,
        }));
        services
    }

    /// Existing sandboxes retain their admitted grants, including explicit
    /// integration additions and the legacy managed route until migrated.
    pub fn connection_services_for_grants(&self, grants: &[ServiceId]) -> Vec<ServiceId> {
        let provider = self.agent_provider.connection_services();
        let mut services = self
            .nyxid_connection_services()
            .into_iter()
            .filter(|service| grants.contains(service))
            .collect::<Vec<_>>();
        for service in grants
            .iter()
            .filter(|service| !provider.contains(service.as_str()))
        {
            if !services.contains(service) {
                services.push(service.clone());
            }
        }
        let provider = match self.model_access {
            ModelAccess::OwnCredentials => provider.subscription,
            ModelAccess::ChronoaiManaged
                if !grants
                    .iter()
                    .any(|service| service.as_str() == provider.managed)
                    && grants
                        .iter()
                        .any(|service| service.as_str() == provider.legacy_managed) =>
            {
                provider.legacy_managed
            }
            ModelAccess::ChronoaiManaged => provider.managed,
        };
        services.push(ServiceId::new(provider));
        services
    }

    /// The common authority calculation for cold and warmed-up admission.
    /// Subscription access never grants the managed LLM service implicitly.
    pub fn credential_services(&self, baseline: &[ServiceId]) -> Vec<ServiceId> {
        let provider = self.agent_provider.connection_services();
        let mut services = self
            .nyxid_services
            .iter()
            .chain(baseline)
            .filter(|service| !provider.contains(service.as_str()))
            .cloned()
            .collect::<BTreeSet<_>>();
        if self.model_access == ModelAccess::ChronoaiManaged {
            services.insert(ServiceId::new(provider.managed));
        }
        services.into_iter().collect()
    }
}
