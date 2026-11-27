use super::*;
use mongodb::bson::Bson;

const DAY_MS: i64 = 86_400_000;

fn current_day() -> Document {
    doc! {"$toLong": {"$floor": {"$divide": [{"$toLong": "$$NOW"}, DAY_MS]}}}
}

impl CreationOutcomes {
    fn coverage_id(&self, epoch: &str) -> String {
        format!("coverage:{}:{epoch}", self.config.deployment)
    }

    fn coverage_filter(&self) -> Document {
        doc! {"_id": self.coverage_id(&self.config.epoch), "document_schema_version": VERSION, "first_day": self.config.first_day}
    }

    pub(super) async fn initialize_coverage(&self) -> Result<(), StoreError> {
        if self
            .records
            .find_one(self.coverage_filter())
            .max_time(IO_BUDGET)
            .await
            .map_err(driver)?
            .is_some()
        {
            return Ok(());
        }
        // Initialization cannot certify a past day after data loss or restore.
        let clock = self
            .database
            .run_command(doc! {"hello":1, "maxTimeMS":750})
            .await
            .map_err(driver)?;
        let now = clock
            .get_datetime("localTime")
            .map_err(|_| StoreError::Malformed("database clock unavailable".into()))?;
        if self.config.first_day != now.timestamp_millis().div_euclid(DAY_MS) + 1 {
            return Err(StoreError::Malformed(
                "creation coverage must start on the next UTC day".into(),
            ));
        }
        self.records.update_one(self.coverage_filter(), doc! {"$setOnInsert": {
            "document_schema_version": VERSION, "first_day": self.config.first_day, "closed_day": self.config.first_day - 1, "admissions": 0_i64
        }}).upsert(true).await.map_err(driver)?;
        Ok(())
    }

    pub(crate) async fn close_elapsed_days(&self) -> Result<(), StoreError> {
        let mut filter = self.coverage_filter();
        let last_closed = doc! {"$subtract": [current_day(), 2_i64]};
        filter.insert("$expr", doc! {"$lt": ["$closed_day", last_closed.clone()]});
        self.records
            .update_one(filter, vec![doc! {"$set": {"closed_day": last_closed}}])
            .await
            .map_err(driver)?;
        Ok(())
    }

    pub(super) async fn admit_day(
        &self,
        day: i64,
        session: &mut ClientSession,
    ) -> Result<Option<DateTime>, mongodb::error::Error> {
        let mut filter = self.coverage_filter();
        filter.insert(
            "$expr",
            doc! {"$and": [
                {"$gte": [day, "$first_day"]}, {"$gt": [day, "$closed_day"]},
                {"$lte": [day, current_day()]}, {"$lte": [current_day(), day + 1]},
            ]},
        );
        let row = self
            .records
            .find_one_and_update(
                filter,
                vec![doc! {"$set": {
                    "admissions": {"$add":["$admissions", 1_i64]}, "admission_clock": "$$NOW"
                }}],
            )
            .return_document(ReturnDocument::After)
            .projection(doc! {"admission_clock":1})
            .max_time(IO_BUDGET)
            .session(session)
            .await?;
        Ok(row.and_then(|row| row.get_datetime("admission_clock").ok().copied()))
    }

    pub(super) async fn is_closed(
        &self,
        records: &Collection<Document>,
        period: &contract::KeyPeriod<'_>,
        session: &mut ClientSession,
    ) -> Result<bool, StatusCode> {
        let filter = if period.epoch == self.config.epoch {
            self.coverage_filter()
        } else {
            // A retired epoch's certificate can prove absence only for days
            // that were durably closed while that epoch was configured.
            doc! {"_id":self.coverage_id(period.epoch), "document_schema_version":VERSION}
        };
        let row = records
            .find_one(filter)
            .projection(doc! {"first_day":1,"closed_day":1})
            .max_time(IO_BUDGET)
            .session(session)
            .await
            .map_err(unavailable)?;
        let Some(row) = row else { return Ok(false) };
        let first_day = row.get_i64("first_day").map_err(unavailable)?;
        let closed_day = match row.get("closed_day") {
            Some(Bson::Int64(day)) => *day,
            Some(Bson::Int32(day)) => i64::from(*day),
            _ => return Err(StatusCode::SERVICE_UNAVAILABLE),
        };
        Ok(period.day >= first_day && period.day <= closed_day)
    }
}

impl CreationOutcomes {
    pub(crate) fn spawn(
        self: std::sync::Arc<Self>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut failed_after = None;
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = shutdown.changed() => break,
                    _ = tick.tick() => {
                        if !matches!(tokio::time::timeout(Duration::from_secs(2), self.close_elapsed_days()).await, Ok(Ok(()))) {
                            tracing::warn!("Sandbox creation coverage closure did not complete");
                        }
                        if !matches!(tokio::time::timeout(Duration::from_secs(5), self.settle_expired_unlinked()).await, Ok(Ok(()))) {
                            tracing::warn!("Sandbox creation terminal settlement did not complete");
                        }
                        if !matches!(tokio::time::timeout(Duration::from_secs(5), self.settle_failed_linked(&mut failed_after)).await, Ok(Ok(()))) {
                            tracing::warn!("Sandbox creation failed-operation settlement did not complete");
                        }
                    }
                }
            }
        })
    }
}
