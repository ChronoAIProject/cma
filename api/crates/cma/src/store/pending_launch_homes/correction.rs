use super::*;
use layer2_core::admission::PendingHomesChange;

impl PendingLaunchHomes {
    pub(in crate::store) async fn correct(
        &self,
        observed_pending: u64,
        actor: &SubjectId,
        reason: &str,
        dry_run: bool,
        now: u64,
    ) -> Result<PendingHomesReconciliation, StoreError> {
        if !self.active() {
            return Ok(PendingHomesReconciliation {
                dry_run,
                ..Default::default()
            });
        }
        let id = format!("pending_launch_homes_change:{}", uuid::Uuid::new_v4());
        match self
            .transaction(Operation::Correct(Correction {
                observed: observed_pending,
                actor,
                reason,
                dry_run,
                now,
                id: &id,
            }))
            .await?
        {
            Outcome::Correction(result) => Ok(result),
            _ => unreachable!("correction transaction returns correction"),
        }
    }

    pub(super) async fn correct_in(
        &self,
        request: &Correction<'_>,
        session: &mut ClientSession,
    ) -> Result<PendingHomesReconciliation, Error> {
        let Correction {
            observed: observed_pending,
            actor,
            reason,
            dry_run,
            now,
            id,
        } = *request;
        let observation = self.observe_in(session).await?;
        let mut result = PendingHomesReconciliation {
            previous_pending_launch_homes: observation.pending,
            expected_pending_launch_homes: observation.expected,
            pending_launch_homes: observation.pending,
            dry_run,
            corrected: false,
        };
        if dry_run {
            return Ok(result);
        }
        if observed_pending != observation.pending {
            return Err(StoreError::RevisionConflict {
                expected: observed_pending,
                actual: observation.pending,
            }
            .into());
        }
        let change = PendingHomesChange {
            at: now,
            actor: actor.clone(),
            reason: reason.into(),
            old_pending_launch_homes: observation.pending,
            new_pending_launch_homes: observation.expected,
        };
        let record = change.activity_record()?;
        let settings = self
            .settings
            .find_one(doc! {"_id": "platform"})
            .max_time(QUERY_TIME)
            .session(&mut *session)
            .await?;
        if let Some(settings) = settings {
            super::super::admission_ceilings::from_document(&settings)?;
        }
        // All ordinary reservation, transfer and release writers use this fence.
        super::super::launch_home_capacity::fence(&self.admissions, session)
            .await
            .map_err(capacity_error)?;
        let mut filter = fence_key();
        if observation.pending == 0 {
            filter.insert(
                "$or",
                mongodb::bson::bson!([
                    {"pending_launch_homes": 0_i64}, {"pending_launch_homes": {"$exists": false}}
                ]),
            );
        } else {
            filter.insert("pending_launch_homes", observation.pending as i64);
        }
        let changed = self
            .admissions
            .update_one(
                filter,
                doc! {"$set": {"pending_launch_homes": observation.expected as i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(StoreError::RevisionConflict {
                expected: observed_pending,
                actual: observation.pending,
            }
            .into());
        }
        let evidence = mongodb::bson::serialize_to_bson(&change)
            .map_err(|_| unavailable("serialize pending home correction"))?;
        self.settings.insert_one(doc! {"_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record": evidence})
            .session(&mut *session).await?;
        self.activity
            .insert_one(ActivityDoc::from_record(&record))
            .session(session)
            .await?;
        result.pending_launch_homes = observation.expected;
        result.corrected = observation.pending != observation.expected;
        Ok(result)
    }
}

pub(super) fn capacity_error(error: super::super::launch_home_capacity::Error) -> Error {
    match error {
        super::super::launch_home_capacity::Error::Driver(error) => Error::Driver(error),
        super::super::launch_home_capacity::Error::Domain(error) => {
            Error::Domain(unavailable(&error.to_string()))
        }
    }
}
