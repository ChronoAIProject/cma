//! New copies retain their own admission; original reads need a source owner.

use layer3_adapter::ingress_budget::{
    sizing, AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy, SharedOwned,
};

use super::SubscriptionRecord;
use crate::auth::persistence::SecretValue;

impl SubscriptionRecord {
    /// This admits only the new snapshot, never a preceding legacy store read.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "admitted subscription refresh integration")
    )]
    pub(in crate::connections) fn copy_admitted(
        source: &SharedOwned<Self>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SharedOwned<Self>, AdmissionRefusal> {
        require_owner(source, policy)?;
        let storage = policy.admit(IngressClass::Data, source.copy_plan()?)?;
        Ok(storage.initialize(|| (**source).clone()))
    }

    /// The decoder receives a separately retained secret without extracting or
    /// prematurely releasing the record's cache. SecretValue keeps zeroization.
    pub(in crate::connections) fn cache_admitted(
        source: &SharedOwned<Self>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<Option<SharedOwned<SecretValue>>, AdmissionRefusal> {
        require_owner(source, policy)?;
        source
            .cache
            .as_ref()
            .map(|cache| {
                let storage = policy.admit(
                    IngressClass::Data,
                    AllocationPlan::checked(
                        "subscription cache copy",
                        &[
                            sizing::shared_owner_bytes::<SecretValue>(),
                            cache.expose().len(),
                        ],
                    )?,
                )?;
                Ok(storage.initialize(|| cache.clone()))
            })
            .transpose()
    }
}

fn require_owner(
    source: &SharedOwned<SubscriptionRecord>,
    policy: &IngressOwnershipPolicy,
) -> Result<(), AdmissionRefusal> {
    if source.shares_custody_domain(policy) {
        Ok(())
    } else {
        Err(AdmissionRefusal::InvalidPlan {
            phase: "subscription record ownership",
        })
    }
}

impl SubscriptionRecord {
    pub(in crate::connections) fn copy_plan(&self) -> Result<AllocationPlan, AdmissionRefusal> {
        // All dynamic fields are String (including SubjectId and SecretValue).
        // String::clone copies len bytes, not the original spare capacity. Options,
        // timestamps and the handoff state are inline in the shared payload layout.
        let pending = self.pending.as_ref();
        AllocationPlan::checked(
            "subscription record copy",
            &[
                sizing::shared_owner_bytes::<SubscriptionRecord>(),
                self.owner.as_str().len(),
                self.incarnation.len(),
                self.cache.as_ref().map_or(0, |cache| cache.expose().len()),
                pending.map_or(0, |pending| pending.intent.len()),
                pending.map_or(0, |pending| pending.device.login_id.len()),
                pending.map_or(0, |pending| pending.device.verification_url.len()),
                pending.map_or(0, |pending| pending.device.user_code.len()),
                pending.map_or(0, |pending| pending.worker.len()),
                self.refresh_lease
                    .as_ref()
                    .map_or(0, |lease| lease.holder.len()),
            ],
        )
    }
}

#[cfg(test)]
#[path = "subscription_store_ingress_tests.rs"]
mod tests;
