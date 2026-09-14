//! Model access and NyxID integrations have separate connection authority.

use std::collections::BTreeSet;

use crate::ServiceId;

use super::{ModelAccess, SandboxProfileContent};

pub const PUBLIC_CODEX_SERVICE: &str = "chrono-llm-public";
/// An internal connection requirement, never a NyxID AgentKey service grant.
pub const CODEX_SUBSCRIPTION_CONNECTION: &str = "cma:codex-subscription";

pub fn is_codex_connection(service: &ServiceId) -> bool {
    matches!(
        service.as_str(),
        "chrono-llm" | PUBLIC_CODEX_SERVICE | CODEX_SUBSCRIPTION_CONNECTION
    )
}

impl SandboxProfileContent {
    /// Existing published revisions may contain a model service in the old
    /// service picker. Interpret it without rewriting the immutable revision.
    pub fn nyxid_connection_services(&self) -> Vec<ServiceId> {
        self.nyxid_services
            .iter()
            .filter(|service| !is_codex_connection(service))
            .cloned()
            .collect()
    }

    /// A stable sequence lets the browser finish NyxID integrations first,
    /// then connect the selected model provider before resource assignment.
    pub fn connection_services(&self) -> Vec<ServiceId> {
        let mut services = self.nyxid_connection_services();
        services.push(ServiceId::new(match self.model_access {
            ModelAccess::ChronoaiManaged => PUBLIC_CODEX_SERVICE,
            ModelAccess::OwnCredentials => CODEX_SUBSCRIPTION_CONNECTION,
        }));
        services
    }

    /// Existing sandboxes retain their admitted grants, including explicit
    /// integration additions and the legacy managed route until migrated.
    pub fn connection_services_for_grants(&self, grants: &[ServiceId]) -> Vec<ServiceId> {
        let mut services = self
            .nyxid_connection_services()
            .into_iter()
            .filter(|service| grants.contains(service))
            .collect::<Vec<_>>();
        for service in grants
            .iter()
            .filter(|service| !is_codex_connection(service))
        {
            if !services.contains(service) {
                services.push(service.clone());
            }
        }
        let provider = match self.model_access {
            ModelAccess::OwnCredentials => CODEX_SUBSCRIPTION_CONNECTION,
            ModelAccess::ChronoaiManaged
                if !grants
                    .iter()
                    .any(|service| service.as_str() == PUBLIC_CODEX_SERVICE)
                    && grants
                        .iter()
                        .any(|service| service.as_str() == "chrono-llm") =>
            {
                "chrono-llm"
            }
            ModelAccess::ChronoaiManaged => PUBLIC_CODEX_SERVICE,
        };
        services.push(ServiceId::new(provider));
        services
    }

    /// The common authority calculation for cold and warmed-up admission.
    /// Subscription access never grants the managed LLM service implicitly.
    pub fn credential_services(&self, baseline: &[ServiceId]) -> Vec<ServiceId> {
        let mut services = self
            .nyxid_services
            .iter()
            .chain(baseline)
            .filter(|service| !is_codex_connection(service))
            .cloned()
            .collect::<BTreeSet<_>>();
        if self.model_access == ModelAccess::ChronoaiManaged {
            services.insert(ServiceId::new(PUBLIC_CODEX_SERVICE));
        }
        services.into_iter().collect()
    }
}
