use super::super::{
    launch_home_capacity::receipt_key, trigger_definitions::decode_launch_aggregate,
};
use super::*;
use futures_util::TryStreamExt;

impl PendingLaunchHomes {
    /// A bounded page on every replica. Discovery never authorizes a release.
    #[cfg(test)]
    pub(crate) async fn sweep(&self, after: Option<Document>) -> Result<SweepReport, StoreError> {
        self.sweep_with_progress(after, None).await
    }

    pub(crate) async fn sweep_with_progress(
        &self,
        after: Option<Document>,
        progress: Option<&std::sync::Mutex<Option<Document>>>,
    ) -> Result<SweepReport, StoreError> {
        if !self.active() {
            return Ok(SweepReport::default());
        }
        self.observe().await?;
        let mut filter = reserved_filter();
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt": after});
        }
        let rows: Vec<Document> = self
            .admissions
            .find(filter)
            .hint(Hint::Name(INDEX.into()))
            .sort(doc! {"state": 1, "_id": 1})
            .limit(BATCH_SIZE)
            .max_time(QUERY_TIME)
            .await
            .map_err(|error| unavailable(&format!("scan pending home receipts: {error}")))?
            .try_collect()
            .await
            .map_err(|error| unavailable(&format!("read pending home receipts: {error}")))?;
        let mut report = SweepReport {
            scanned: rows.len() as u64,
            ..Default::default()
        };
        if rows.len() == BATCH_SIZE as usize {
            report.next = rows
                .last()
                .and_then(|row| row.get_document("_id").ok())
                .cloned();
        }
        for row in rows {
            // Advance before I/O. Cancellation may leave this candidate uncertain;
            // the next full traversal revisits it, while the tail still gets time.
            if let Some(progress) = progress {
                *progress.lock().expect("pending home cursor poisoned") =
                    row.get_document("_id").ok().cloned();
            }
            let result = self.discover_and_repair(&row).await;
            match result {
                Ok(None) => report.missing_launches += 1,
                Ok(Some(released)) => report.released += u64::from(released),
                Err(error) => {
                    tracing::warn!(target: "cma::admission", event = "pending_launch_home_repair_failed", error = %error,
                    "pending Launch home receipt was retained for investigation")
                }
            }
        }
        Ok(report)
    }

    async fn discover_and_repair(&self, row: &Document) -> Result<Option<bool>, StoreError> {
        let launch = receipt_launch(row)?;
        let document = self
            .launches
            .find_one(doc! {"_id": launch})
            .max_time(QUERY_TIME)
            .await
            .map_err(|error| unavailable(&format!("find pending Launch: {error}")))?;
        let Some(document) = document else {
            tracing::warn!(target: "cma::admission", event = "pending_launch_home_missing_launch", launch_id = launch,
                "reserved home receipt has no Launch; operator investigation is required");
            return Ok(None);
        };
        let launch_record =
            decode_launch_aggregate(&document).map_err(|error| unavailable(&error.to_string()))?;
        if !launch_record.state.is_terminal() {
            return Ok(Some(false));
        }
        match self.transaction(Operation::Repair(launch)).await? {
            Outcome::Repaired(released) => Ok(Some(released)),
            _ => unreachable!("repair transaction returns repair outcome"),
        }
    }

    pub(super) async fn repair_in(
        &self,
        launch: &str,
        session: &mut ClientSession,
    ) -> Result<bool, Error> {
        self.require_activation(session).await?;
        let mut reserved = receipt_key(launch);
        reserved.extend(reserved_filter());
        let row = self
            .admissions
            .find_one(reserved)
            .hint(Hint::Name(INDEX.into()))
            .max_time(QUERY_TIME)
            .session(&mut *session)
            .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        require_version(&row).map_err(StoreError::Malformed)?;
        match row.get_str("state").ok() {
            Some("consumed" | "released" | "reused") => return Ok(false),
            Some("reserved") => {}
            _ => {
                return Err(
                    StoreError::Malformed("invalid pending home receipt state".into()).into(),
                )
            }
        }
        if receipt_launch(&row)? != launch {
            return Err(unavailable("pending home receipt identity mismatch").into());
        }
        let document = self
            .launches
            .find_one(doc! {"_id": launch})
            .max_time(QUERY_TIME)
            .session(&mut *session)
            .await?;
        let Some(document) = document else {
            tracing::warn!(target: "cma::admission", event = "pending_launch_home_missing_launch", launch_id = launch,
                "reserved home receipt has no Launch; operator investigation is required");
            return Ok(false);
        };
        let current =
            decode_launch_aggregate(&document).map_err(|error| unavailable(&error.to_string()))?;
        if row.get_str("owner").ok() != Some(current.owner.as_str()) {
            return Err(unavailable("pending home receipt owner mismatch").into());
        }
        if !current.state.is_terminal() {
            return Ok(false);
        }
        super::super::launch_home_capacity::fence(&self.admissions, session)
            .await
            .map_err(super::correction::capacity_error)?;
        let mut filter = receipt_key(launch);
        filter.insert("state", "reserved");
        let released = self
            .admissions
            .update_one(filter, doc! {"$set": {"state": "released"}})
            .session(&mut *session)
            .await?;
        // A normal release or another replica already settled this receipt.
        if released.matched_count == 0 {
            return Ok(false);
        }
        let counter = self
            .admissions
            .find_one(fence_key())
            .session(&mut *session)
            .await?;
        if pending_homes(counter.as_ref())? == 0 {
            return Err(
                unavailable("pending home counter cannot cover a terminal reservation").into(),
            );
        }
        let mut filter = fence_key();
        filter.insert("pending_launch_homes", doc! {"$gte": 1_i64});
        let changed = self
            .admissions
            .update_one(filter, doc! {"$inc": {"pending_launch_homes": -1_i64}})
            .session(session)
            .await?;
        if changed.matched_count != 1 {
            return Err(unavailable("pending home decrement did not match").into());
        }
        Ok(true)
    }
}

fn receipt_launch(row: &Document) -> Result<&str, StoreError> {
    require_version(row).map_err(StoreError::Malformed)?;
    let key = row
        .get_document("_id")
        .map_err(|_| unavailable("invalid pending home receipt key"))?;
    if key.len() != 2 || key.get_str("scope").ok() != Some("launch_home_capacity") {
        return Err(unavailable("invalid pending home receipt scope"));
    }
    key.get_str("launch")
        .ok()
        .filter(|launch| !launch.is_empty())
        .ok_or_else(|| unavailable("invalid pending home receipt Launch"))
}
