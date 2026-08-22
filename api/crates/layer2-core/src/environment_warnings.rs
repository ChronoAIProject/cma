//! Non-blocking notices raised after an environment profile is stored.

use crate::authz::Caller;
use crate::environment::ServiceId;
use crate::environment_service::EnvironmentProfileService;
use crate::size::SessionSize;

impl EnvironmentProfileService {
    /// Note a selection the catalogue does not currently grant.
    ///
    /// A warning and never a refusal. Refusing here would turn a momentary
    /// catalogue outage into a caller unable to save a profile, and would
    /// refuse a selection that may be granted before the profile is used.
    pub(super) async fn warn_ungranted(
        &self,
        caller: &Caller,
        credential: Option<&str>,
        selected: &[ServiceId],
    ) {
        let Ok(availability) = self.service_availability(caller, credential).await else {
            return;
        };
        let Some(granted) = availability.grants() else {
            return;
        };
        for id in selected {
            if self.baseline.services().contains(id) {
                continue;
            }
            if !granted.iter().any(|service| &service.id == id) {
                tracing::warn!(
                    subject = %caller.subject,
                    service = %id.0,
                    "a profile names a service this owner has not granted; \
                     sessions started from it will be refused until they do"
                );
            }
        }
    }

    /// Note a stated size outside the deployment's current policy.
    ///
    /// Profiles remain writable because the policy can move after a profile is
    /// saved. Creation is the moment that refuses a size which remains outside
    /// the bounds.
    pub(super) async fn warn_out_of_bounds(&self, caller: &Caller, size: Option<SessionSize>) {
        let Some(size) = size else {
            return;
        };
        let settings = self.settings.for_provisioning().await;
        if let Err(refusal) = settings.size_bounds.check(size) {
            tracing::warn!(
                subject = %caller.subject,
                dimension = %refusal.dimension,
                requested = refusal.requested,
                minimum = refusal.minimum,
                maximum = refusal.maximum,
                "a profile names a size outside this deployment's bounds; \
                 sessions started from it will be refused until the size or the bounds change"
            );
        }
    }
}
