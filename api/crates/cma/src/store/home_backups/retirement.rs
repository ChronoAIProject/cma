//! Retirement and late schedule discovery serialize on the schedule document.
use super::*;
use mongodb::options::ReturnDocument;

#[async_trait::async_trait]
impl HomeBackupRetirementStore for MongoHomeBackupStore {
    async fn retire_schedule(
        &self,
        seed: &HomeBackupSchedule,
    ) -> Result<HomeBackupSchedule, HomeBackupError> {
        validate_seed(seed)?;
        let sentinel = seed.retiring()?;
        self.schedules
            .update_one(
                doc! {"_id": &seed.id},
                doc! {"$setOnInsert": schedule_document(&sentinel).map_err(malformed)?},
            )
            .upsert(true)
            .await
            .map_err(failed)?;
        let previous = self
            .get_schedule(&seed.id)
            .await?
            .ok_or(HomeBackupError::Conflict)?;
        if previous.home != seed.home {
            return Err(HomeBackupError::Conflict);
        }
        let next = previous.retiring()?;
        if previous.state == HomeBackupScheduleState::Retiring {
            return Ok(previous);
        }
        let mut fields = schedule_document(&next).map_err(malformed)?;
        fields.remove("_id");
        let row = self
            .schedules
            .find_one_and_update(
                doc! {
                    "_id": &previous.id,
                    "state": "active",
                    "revision": codec::signed(previous.revision).map_err(malformed)?,
                    "next_fence": codec::signed(previous.next_fence).map_err(malformed)?,
                },
                doc! {"$set": fields},
            )
            .return_document(ReturnDocument::After)
            .await
            .map_err(failed)?
            .ok_or(HomeBackupError::Conflict)?;
        read_schedule(row).map_err(malformed)
    }
}

fn validate_seed(seed: &HomeBackupSchedule) -> Result<(), HomeBackupError> {
    seed.validate()?;
    if seed.state != HomeBackupScheduleState::Active
        || seed.revision != 1
        || seed.next_fence != 1
        || seed.lease.is_some()
        || seed.in_flight.is_some()
        || seed.last_ready_at.is_some()
        || seed.last_ready_backup_id.is_some()
    {
        return Err(HomeBackupError::Conflict);
    }
    Ok(())
}
