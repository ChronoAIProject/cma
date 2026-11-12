//! Operator-only bounded observations. No periodic writes touch settings.
use super::*;
use layer2_core::admission::{AdmissionCapacityCounts, AdmissionCeilings, CapacityCount};
use mongodb::options::Hint;
use std::time::Duration;

impl MongoAdmissionCeilingsStore {
    pub(super) async fn read_counts(
        &self,
        ceilings: &AdmissionCeilings,
        session: &mut ClientSession,
    ) -> Result<(AdmissionCeilings, AdmissionCapacityCounts), ApplyError> {
        let mut effective = ceilings.clone();
        effective.retained_homes = ceilings.effective_retained_homes(self.deployment_limit);
        let mut counts = AdmissionCapacityCounts::default();
        let fence = self
            .database
            .collection::<Document>("sandbox_admissions")
            .find_one(doc! {"_id": {"scope": "global_sandbox_capacity"}})
            .max_time(Duration::from_secs(3))
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        counts.pending_launch_homes =
            super::super::launch_home_capacity::pending_homes(fence.as_ref())
                .map_err(ApplyError::Domain)?;
        if let Some(limit) = ceilings.registrations {
            let row = self
                .database
                .collection::<Document>("sandbox_admissions")
                .find_one(doc! {"_id": {"scope": "registration_capacity"}})
                .max_time(Duration::from_secs(3))
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
            let used = row
                .as_ref()
                .map(super::super::registration_capacity::registration_count)
                .transpose()
                .map_err(ApplyError::Domain)?
                .unwrap_or(0);
            counts.registrations = Some(observation(used.min(u64::from(limit).max(1)), limit));
        }
        if let Some(limit) = effective.retained_homes {
            let count = self
                .database
                .collection::<Document>("sandboxes")
                .count_documents(doc! {"retired_at": Bson::Null})
                .hint(Hint::Name("sandboxes_global_live".into()))
                .limit(u64::from(limit).max(1))
                .max_time(Duration::from_secs(3))
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
            counts.retained_homes = Some(observation(
                count
                    .saturating_add(counts.pending_launch_homes)
                    .min(u64::from(limit).max(1)),
                limit,
            ));
        }
        if let Some(limit) = ceilings.active_sessions {
            let count = self
                .database
                .collection::<Document>("agents")
                .count_documents(doc! {"status": {"$in": ["working", "waiting_approval"]}})
                .hint(Hint::Keys(doc! {"status": 1, "_id": 1}))
                .limit(u64::from(limit).max(1))
                .max_time(Duration::from_secs(3))
                .session(&mut *session)
                .await
                .map_err(ApplyError::Driver)?;
            counts.active_sessions = Some(observation(count, limit));
        }
        Ok((effective, counts))
    }
}

fn observation(observed: u64, limit: u32) -> CapacityCount {
    let bound = u64::from(limit).max(1);
    CapacityCount {
        observed,
        bound,
        saturated: observed >= bound,
    }
}
