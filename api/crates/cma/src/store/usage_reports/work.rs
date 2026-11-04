use super::*;
use layer2_core::sandbox_metrics::{Report, ReportStatus, Row, Sort};
use mongodb::options::ReturnDocument;
pub(crate) struct ReportLease {
    pub report: Report,
    pub holder: String,
    pub generation: i64,
    pub slot: String,
}
impl ReportingStore {
    pub(crate) async fn claim_report(
        &self,
        slot: &str,
        holder: &str,
    ) -> Result<Option<ReportLease>> {
        let at = now();
        self.check_circuit(at)?;
        self.prune_cache(at).await?;
        if !self.lease(slot, holder, 70).await? {
            return Ok(None);
        }
        let d=self.collection(JOBS).find_one_and_update(doc!{"logical_expires_at":{"$gt":at as i64},"$or":[{"state":"pending"},{"state":"running","lease_until":{"$lte":at as i64}}]},
            doc!{"$set":{"state":"running","lease_holder":holder,"lease_until":(at+70) as i64,"bytes":0_i64},"$inc":{"generation":1_i64}})
            .sort(doc!{"created_at":1}).return_document(ReturnDocument::After).max_time(DB_DEADLINE).await.map_err(unavailable)?;
        let Some(d) = d else {
            self.release(slot, holder).await?;
            return Ok(None);
        };
        let mut report: Report = decode(&d)?;
        report.as_of = at;
        report.measurement_cutoff =
            at.saturating_sub(layer2_core::sandbox_metrics::FINALIZATION_DELAY_SECONDS);
        report.effective_range = report.query.effective_range(at);
        report.expires_at = at + layer2_core::sandbox_metrics::REPORT_TTL_SECONDS;
        let lease = ReportLease {
            report,
            holder: holder.into(),
            generation: d.get_i64("generation").map_err(unavailable)?,
            slot: slot.into(),
        };
        // Cleanup is fenced in the same read-model transaction as the lease.
        // A delayed former worker cannot erase its successor's cached rows.
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        session.start_transaction().await.map_err(unavailable)?;
        if self
            .collection(JOBS)
            .update_one(lease.fence(), doc! {"$set":{"bytes":0_i64,"record":encode(&lease.report)?,"logical_expires_at":lease.report.expires_at as i64,"expires_at":expires(lease.report.expires_at+86_400)}})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .matched_count
            != 1
        {
            return Err(MetricsError::Unavailable);
        }
        for collection in [ROWS, CHARTS] {
            self.collection(collection)
                .delete_many(doc! {"report_id":&lease.report.report_id})
                .session(&mut session)
                .await
                .map_err(unavailable)?;
        }
        session.commit_transaction().await.map_err(unavailable)?;
        Ok(Some(lease))
    }
    async fn prune_cache(&self, at: u64) -> Result<()> {
        use futures_util::TryStreamExt;
        let mut cursor = self
            .collection(JOBS)
            .find(doc! {"reserved":true,"logical_expires_at":{"$lte":at as i64}})
            .limit(8)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        while let Some(d) = cursor.try_next().await.map_err(unavailable)? {
            let id = d.get_str("_id").map_err(unavailable)?;
            for collection in [ROWS, CHARTS] {
                let result=self.db.run_command(doc! {"delete":collection,"deletes":[{"q":{"report_id":id},"limit":0}],"maxTimeMS":2000_i64})
                    .await.map_err(unavailable)?;
                if result.get_array("writeErrors").is_ok_and(|v| !v.is_empty())
                    || result.contains_key("writeConcernError")
                {
                    return Err(MetricsError::Unavailable);
                }
            }
            // Admission never releases a byte reservation before cache data
            // has been removed, even when the asynchronous TTL monitor lags.
            self.collection(JOBS)
                .update_one(
                    doc! {"_id":id},
                    doc! {"$set":{"reserved":false,"bytes":0_i64}},
                )
                .await
                .map_err(unavailable)?;
        }
        Ok(())
    }

    pub(crate) async fn write_report_chunk(
        &self,
        lease: &ReportLease,
        rows: &[Row],
        charts: Vec<Document>,
    ) -> Result<()> {
        let rows = rows
            .iter()
            .map(|row| row_document(&lease.report, row))
            .collect::<Result<Vec<_>>>()?;
        let bytes = rows.iter().chain(&charts).try_fold(0_i64, |n, d| {
            mongodb::bson::serialize_to_vec(d)
                .map(|v| n + v.len() as i64)
                .map_err(unavailable)
        })?;
        if bytes > self.budgets.report_bytes {
            return Err(MetricsError::Busy);
        }
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        session.start_transaction().await.map_err(unavailable)?;
        let mut fence = lease.fence();
        fence.insert("bytes", doc! {"$lte":self.budgets.report_bytes-bytes});
        fence.insert(
            "$expr",
            doc! {"$lte":[{"$add":["$bytes",bytes]},"$reserved_bytes"]},
        );
        let updated = self
            .collection(JOBS)
            .update_one(fence, doc! {"$inc":{"bytes":bytes}})
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if updated.matched_count != 1 {
            return Err(MetricsError::Busy);
        }
        if !rows.is_empty() {
            self.collection(ROWS)
                .insert_many(rows)
                .session(&mut session)
                .await
                .map_err(unavailable)?;
        }
        if !charts.is_empty() {
            self.collection(CHARTS)
                .insert_many(charts)
                .session(&mut session)
                .await
                .map_err(unavailable)?;
        }
        session.commit_transaction().await.map_err(unavailable)?;
        Ok(())
    }
    pub(crate) async fn finish_report(
        &self,
        lease: &ReportLease,
        mut report: Report,
        success: bool,
    ) -> Result<()> {
        report.status = if success {
            ReportStatus::Ready
        } else {
            ReportStatus::Failed
        };
        report.report_complete = success;
        report.expires_at = now() + layer2_core::sandbox_metrics::REPORT_TTL_SECONDS;
        let state = if success { "ready" } else { "failed" };
        let result = self
            .collection(JOBS)
            .update_one(
                lease.fence(),
                doc! {"$set":{"record":encode(&report)?,"state":state,"lease_until":0_i64,"logical_expires_at":report.expires_at as i64,"expires_at":expires(report.expires_at+86_400)}},
            )
            .await
            .map_err(unavailable)?;
        self.release(&lease.slot, &lease.holder).await?;
        if result.matched_count != 1 {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }
}
impl ReportLease {
    fn fence(&self) -> Document {
        doc! {"_id":&self.report.report_id,"generation":self.generation,"lease_holder":&self.holder,"state":"running","lease_until":{"$gt":now() as i64}}
    }
}
fn row_document(report: &Report, row: &Row) -> Result<Document> {
    let mut sorts = Document::new();
    let mut nulls = Document::new();
    for sort in Sort::ALL {
        let value = match sort {
            Sort::Name => Some(row.name.to_lowercase().into()),
            Sort::Owner => Some(row.owner.subject.clone().into()),
            Sort::Status => Some(row.status.clone().into()),
            Sort::LastActivityAt => row.last_activity_at.map(|v| (v as i64).into()),
            Sort::CpuMeanCores => row.measured.cpu.mean.map(Into::into),
            Sort::MemoryMeanBytes => row.measured.memory.mean.map(Into::into),
            Sort::DiskLatestBytes => row.measured.disk.latest.map(Into::into),
            Sort::AllocatedCpuCoreHours => row
                .allocated_period
                .complete
                .then(|| row.allocated_period.cpu_core_hours.into()),
            Sort::AllocatedMemoryGibHours => row
                .allocated_period
                .complete
                .then(|| row.allocated_period.memory_gib_hours.into()),
            Sort::AllocatedDiskGibHours => row
                .allocated_period
                .complete
                .then(|| row.allocated_period.disk_gib_hours.into()),
        };
        nulls.insert(sort.key(), value.is_none());
        sorts.insert(sort.key(), value.unwrap_or(mongodb::bson::Bson::Null));
    }
    Ok(
        doc! {"_id":format!("{}:{}",report.report_id,row.sandbox_id),"document_schema_version":1_i64,"report_id":&report.report_id,
        "sandbox_id":&row.sandbox_id,"owner":&row.owner.subject,"sorts":sorts,"nulls":nulls,"expires_at":expires(report.expires_at+60),"record":encode(row)?},
    )
}
