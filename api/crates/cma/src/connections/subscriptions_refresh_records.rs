//! Candidate storage is separate from the engine's authority and CAS decisions.

use layer3_adapter::ingress_budget::{
    AdmissionRefusal, AllocationPlan, IngressClass, IngressOwnershipPolicy, SharedOwned,
};

use super::{CacheUpdate, Duration, SecretValue, SubscriptionRecord, SystemTime};
use crate::connections::subscription_store::RefreshLease;

pub(super) enum RecordChange<'a> {
    Claim {
        holder: &'a str,
        lease_lifetime: Duration,
    },
    Finish(CacheUpdate<'a, SecretValue>),
}

impl RecordChange<'_> {
    pub(super) fn build(self, source: &SubscriptionRecord) -> SubscriptionRecord {
        let mut next = source.clone();
        next.revision += 1;
        match self {
            Self::Claim {
                holder,
                lease_lifetime,
            } => {
                next.refresh_lease = Some(RefreshLease {
                    holder: holder.to_owned(),
                    expires_at: SystemTime::now() + lease_lifetime,
                });
            }
            Self::Finish(cache) => {
                next.refresh_lease = None;
                match cache {
                    CacheUpdate::Replace(cache) => next.cache = Some(cache.clone()),
                    CacheUpdate::Clear => next.cache = None,
                    CacheUpdate::Keep => {}
                }
            }
        }
        next
    }

    fn plan(&self, source: &SubscriptionRecord) -> Result<AllocationPlan, AdmissionRefusal> {
        // Build clones the whole old record before replacing a lease or cache.
        // Keep that complete copy allowance while its replacement is allocated;
        // this includes both backing owners, whose allocation follows build.
        let replacement = match self {
            Self::Claim { holder, .. } => holder.len(),
            Self::Finish(CacheUpdate::Replace(cache)) => cache.expose().len(),
            Self::Finish(CacheUpdate::Clear | CacheUpdate::Keep) => 0,
        };
        AllocationPlan::checked(
            "subscription refresh candidate",
            &[source.copy_plan()?.requested_bytes(), replacement],
        )
    }
}

impl SubscriptionRecord {
    /// The caller owns unique holder generation; this admits only its new copy
    /// and candidate record. A candidate cannot authorize its own CAS write.
    pub(in crate::connections) fn claim_admitted(
        source: &SharedOwned<Self>,
        holder: &SharedOwned<String>,
        lease_lifetime: Duration,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SharedOwned<Self>, AdmissionRefusal> {
        require_owner(source, policy)?;
        require_owner(holder, policy)?;
        RecordChange::Claim {
            holder,
            lease_lifetime,
        }
        .initialize(source, policy)
    }

    /// The shared engine selects cache replacement, clearing or preservation
    /// only after its existing incarnation and holder checks.
    pub(in crate::connections) fn finish_admitted(
        source: &SharedOwned<Self>,
        cache: CacheUpdate<'_, SharedOwned<SecretValue>>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SharedOwned<Self>, AdmissionRefusal> {
        require_owner(source, policy)?;
        let cache = match cache {
            CacheUpdate::Replace(cache) => {
                require_owner(cache, policy)?;
                CacheUpdate::Replace(&**cache)
            }
            CacheUpdate::Clear => CacheUpdate::Clear,
            CacheUpdate::Keep => CacheUpdate::Keep,
        };
        RecordChange::Finish(cache).initialize(source, policy)
    }
}

impl RecordChange<'_> {
    fn initialize(
        self,
        source: &SubscriptionRecord,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SharedOwned<SubscriptionRecord>, AdmissionRefusal> {
        let storage = policy.admit(IngressClass::Data, self.plan(source)?)?;
        Ok(storage.initialize(|| self.build(source)))
    }
}

fn require_owner<T>(
    value: &SharedOwned<T>,
    policy: &IngressOwnershipPolicy,
) -> Result<(), AdmissionRefusal> {
    if value.shares_custody_domain(policy) {
        Ok(())
    } else {
        Err(AdmissionRefusal::InvalidPlan {
            phase: "subscription refresh candidate ownership",
        })
    }
}

#[cfg(test)]
#[path = "subscriptions_refresh_records_tests.rs"]
mod tests;
