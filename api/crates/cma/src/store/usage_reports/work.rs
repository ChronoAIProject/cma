use super::{aggregate_projections, *};
use crate::store::transaction_scope as tx;
use layer2_core::sandbox_metrics::{Report, ReportStatus, Row, Sort};
use mongodb::options::ReturnDocument;
pub(crate) struct ReportLease {
    pub report: Report,
    pub holder: String,
    pub generation: i64,
    pub slot: String,
    pub view: Option<ViewLease>,
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
        // Retained jobs use different states so predecessor workers cannot
        // claim them and replace their fixed retention with the legacy TTL.
        let d=self.collection(JOBS).find_one_and_update(doc!{"logical_expires_at":{"$gt":at as i64},"$or":[
            {"state":"pending"},{"state":"running","lease_until":{"$lte":at as i64}},
            {"kind":views::VIEW_KIND,"state":"view_pending"},
            {"kind":views::VIEW_KIND,"state":"view_running","lease_until":{"$lte":at as i64}}]},
            vec![doc!{"$set":{"state":{"$cond":[{"$eq":["$kind",views::VIEW_KIND]},"view_running","running"]},
                "lease_holder":holder,"lease_until":(at+70) as i64,"bytes":0_i64,
                "generation":{"$add":["$generation",1_i64]}}}])
            .sort(doc!{"created_at":1}).return_document(ReturnDocument::After).max_time(DB_DEADLINE).await.map_err(unavailable)?;
        let Some(d) = d else {
            self.release(slot, holder).await?;
            return Ok(None);
        };
        let mut report: Report = decode(&d)?;
        tracing::debug!(
            report_id = %report.report_id,
            queue_delay_seconds = at.saturating_sub(report.as_of),
            "sandbox report claimed"
        );
        report.as_of = at;
        report.measurement_cutoff =
            at.saturating_sub(layer2_core::sandbox_metrics::FINALIZATION_DELAY_SECONDS);
        report.effective_range = report.query.effective_range(at);
        let view = if d.get_str("kind").ok() == Some(views::VIEW_KIND) {
            report.expires_at = u64::try_from(d.get_i64("retain_until").map_err(unavailable)?)
                .map_err(unavailable)?;
            Some(ViewLease {
                key: d.get_str("view_key").map_err(unavailable)?.into(),
                revision: d.get_i64("view_revision").map_err(unavailable)?,
            })
        } else {
            report.expires_at = at + layer2_core::sandbox_metrics::REPORT_TTL_SECONDS;
            None
        };
        let lease = ReportLease {
            report,
            holder: holder.into(),
            generation: d.get_i64("generation").map_err(unavailable)?,
            slot: slot.into(),
            view,
        };
        // Cleanup is fenced in the same read-model transaction as the lease.
        // A delayed former worker cannot erase its successor's cached rows.
        let previous_projection = d.contains_key(aggregate_projections::GENERATION_FIELD);
        tx::run(&self.client, (self, &lease, previous_projection), |&(store, lease, previous_projection), session| {
            Box::pin(async move {
        if store
            .collection(JOBS)
            .update_one(lease.fence(), doc! {"$set":{"bytes":0_i64,"row_count":0_i64,"chart_count":0_i64,"record":encode(&lease.report)?,"logical_expires_at":lease.report.expires_at as i64,"expires_at":if lease.view.is_some() { mongodb::bson::Bson::Null } else { expires(lease.report.expires_at+86_400).into() }},
                "$unset":{aggregate_projections::GENERATION_FIELD:"",aggregate_projections::MANIFEST_FIELD:""}})
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?
            .matched_count
            != 1
        {
            return Err(MetricsError::Unavailable.into());
        }
        let projections = previous_projection.then_some(aggregate_projections::COLLECTION);
        for collection in [ROWS, CHARTS].into_iter().chain(projections) {
            store.collection(collection)
                .delete_many(doc! {"report_id":&lease.report.report_id})
                .session(&mut *session)
                .await
                .map_err(tx::mongo)?;
        }
                Ok(())
            })
        }).await?;
        Ok(Some(lease))
    }
    pub(super) async fn prune_cache(&self, at: u64) -> Result<()> {
        use futures_util::TryStreamExt;
        let mut cursor = self
            .collection(JOBS)
            .find(doc! {"reserved":true,"$or":[{"logical_expires_at":{"$lte":at as i64}},{"kind":views::VIEW_KIND,"retire_after":{"$lte":at as i64}}]})
            .limit(8)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        while let Some(d) = cursor.try_next().await.map_err(unavailable)? {
            let id = d.get_str("_id").map_err(unavailable)?;
            if !self.retire_expired_report(&d).await? {
                continue;
            }
            let projected = d.contains_key(aggregate_projections::GENERATION_FIELD);
            for collection in [ROWS, CHARTS] {
                self.delete_report_cache(collection, doc! {"report_id":id})
                    .await?;
            }
            if projected {
                // Maintain contributor discovery until all derived pages are gone.
                self.delete_report_cache(
                    aggregate_projections::COLLECTION,
                    doc! {"report_id":id,"kind":{"$ne":"contributor"}},
                )
                .await?;
                self.delete_report_cache(
                    aggregate_projections::COLLECTION,
                    doc! {"report_id":id,"kind":"contributor"},
                )
                .await?;
            }
            let mut released = doc! {"reserved":false,"bytes":0_i64};
            if projected || d.get_str("kind").ok() == Some(views::VIEW_KIND) {
                released.insert("expires_at", expires(at + 60));
            }
            self.collection(JOBS)
                .update_one(
                    doc! {"_id":id,"generation":d.get_i64("generation").map_err(unavailable)?,
                    "state":retired_state(&d)},
                    doc! {"$set":released},
                )
                .await
                .map_err(unavailable)?;
        }
        Ok(())
    }

    async fn delete_report_cache(&self, collection: &str, filter: Document) -> Result<()> {
        let result = self
            .db
            .run_command(doc! {"delete":collection,
            "deletes":[{"q":filter,"limit":0}],"maxTimeMS":2000_i64})
            .await
            .map_err(unavailable)?;
        if result
            .get_array("writeErrors")
            .is_ok_and(|errors| !errors.is_empty())
            || result.contains_key("writeConcernError")
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }

    pub(super) async fn retire_expired_report(&self, candidate: &Document) -> Result<bool> {
        // A scan is only a candidate: claiming or completing can advance its
        // generation/deadline before deletion. Retire the exact live record
        // first, conflicting with chunk transactions and excluding new claims.
        let projection = candidate
            .get(aggregate_projections::GENERATION_FIELD)
            .cloned()
            .unwrap_or_else(|| doc! {"$exists":false}.into());
        let filter = doc! {"_id":candidate.get_str("_id").map_err(unavailable)?,
        aggregate_projections::GENERATION_FIELD:projection,
        "reserved":true,"generation":candidate.get_i64("generation").map_err(unavailable)?,
        "state":candidate.get_str("state").map_err(unavailable)?,
        "$expr":{"$or":[{"$lte":["$logical_expires_at",server_seconds()]},
            {"$and":[{"$eq":["$kind",views::VIEW_KIND]},
                {"$lte":[{"$ifNull":["$retire_after",i64::MAX]},server_seconds()]}]}]}};
        let updated = self
            .collection(JOBS)
            .update_one(filter, doc! {"$set":{"state":retired_state(candidate)}})
            .await
            .map_err(unavailable)?;
        Ok(updated.matched_count == 1)
    }

    #[cfg(test)]
    pub(crate) async fn write_report_chunk(
        &self,
        lease: &ReportLease,
        rows: &[Row],
        charts: Vec<Document>,
    ) -> Result<()> {
        let charts = charts
            .iter()
            .map(|d| mongodb::bson::RawDocumentBuf::try_from(d).map_err(unavailable))
            .collect::<Result<Vec<_>>>()?;
        self.write_prepared_report_chunk(lease, rows, charts).await
    }

    pub(crate) async fn write_prepared_report_chunk(
        &self,
        lease: &ReportLease,
        rows: &[Row],
        charts: Vec<mongodb::bson::RawDocumentBuf>,
    ) -> Result<()> {
        let rows = rows
            .iter()
            .map(|row| {
                mongodb::bson::RawDocumentBuf::try_from(&row_document(&lease.report, row)?)
                    .map_err(unavailable)
            })
            .collect::<Result<Vec<_>>>()?;
        // These exact bytes are also inserted below. Counting preencoded
        // documents avoids rebuilding large chart trees merely to budget them.
        let bytes = rows.iter().chain(&charts).try_fold(0_i64, |n, d| {
            n.checked_add(d.as_bytes().len() as i64)
                .ok_or(MetricsError::Busy)
        })?;
        if bytes > self.budgets.report_bytes {
            return Err(MetricsError::Busy);
        }
        tx::run(
            &self.client,
            (self, lease, &rows, &charts, bytes),
            |&(store, lease, rows, charts, bytes), session| {
                Box::pin(async move {
                    let row_count = rows.len() as i64;
                    let chart_count = charts.len() as i64;
                    // Keep the server-time lease expression and byte expression together;
                    // inserting another $expr into the lease filter would discard expiry.
                    let fence = doc! {"$and":[lease.fence(), {
                    aggregate_projections::GENERATION_FIELD:{"$exists":false},
                    "bytes":{"$lte":store.budgets.report_bytes-bytes},
                    "$expr":{"$lte":[{"$add":["$bytes",bytes]},"$reserved_bytes"]}}]};
                    let updated = store
            .collection(JOBS)
            .update_one(
                fence,
                doc! {"$inc":{"bytes":bytes,"row_count":row_count,"chart_count":chart_count}},
            )
            .session(&mut *session)
            .await
            .map_err(tx::mongo)?;
                    if updated.matched_count != 1 {
                        return Err(MetricsError::Busy.into());
                    }
                    if !rows.is_empty() {
                        store
                            .collection(ROWS)
                            .clone_with_type::<mongodb::bson::RawDocumentBuf>()
                            .insert_many(rows)
                            .session(&mut *session)
                            .await
                            .map_err(tx::mongo)?;
                    }
                    if !charts.is_empty() {
                        store
                            .collection(CHARTS)
                            .clone_with_type::<mongodb::bson::RawDocumentBuf>()
                            .insert_many(charts)
                            .session(&mut *session)
                            .await
                            .map_err(tx::mongo)?;
                    }
                    Ok(())
                })
            },
        )
        .await
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
        if lease.view.is_some() {
            report.expires_at = lease.report.expires_at;
            return self.finish_view(lease, &report, success).await;
        }
        report.expires_at = now() + layer2_core::sandbox_metrics::REPORT_TTL_SECONDS;
        let state = if success { "ready" } else { "failed" };
        let result = self
            .collection(JOBS)
            .update_one(
                lease.fence(),
                // A terminal report cannot grow: retain its actual cache bytes,
                // not its maximum work reservation, throughout the cache TTL.
                vec![doc! {"$set":{"record":{"$literal":encode(&report)?},"state":state,"lease_until":0_i64,"reserved_bytes":"$bytes","logical_expires_at":report.expires_at as i64,"expires_at":{"$cond":[{"$ne":[{"$type":"$projection_generation"},"missing"]},mongodb::bson::Bson::Null,expires(report.expires_at+86_400)]}}}],
            )
            .await
            .map_err(unavailable)?;
        self.release(&lease.slot, &lease.holder).await?;
        if result.matched_count != 1 {
            return Err(MetricsError::Unavailable);
        }
        self.notifications.finished(&report.report_id);
        Ok(())
    }
}
impl ReportLease {
    pub(super) fn fence(&self) -> Document {
        doc! {"_id":&self.report.report_id,"generation":self.generation,"lease_holder":&self.holder,
        "state":if self.view.is_some() {"view_running"} else {"running"},
        "$expr":{"$and":[{"$gt":["$lease_until",server_seconds()]},
            {"$gt":["$logical_expires_at",server_seconds()]}]}}
    }
}
fn server_seconds() -> Document {
    doc! {"$toLong":{"$divide":[{"$toLong":"$$NOW"},1000_i64]}}
}
fn retired_state(job: &Document) -> &'static str {
    if job.get_str("kind").ok() == Some(views::VIEW_KIND) {
        "view_retired"
    } else {
        "retired"
    }
}
pub(super) fn row_document(report: &Report, row: &Row) -> Result<Document> {
    let (sorts, nulls) = row_sort_keys(row);
    Ok(
        doc! {"_id":format!("{}:{}",report.report_id,row.sandbox_id),"document_schema_version":1_i64,"report_id":&report.report_id,
        "sandbox_id":&row.sandbox_id,"owner":&row.owner.subject,"sorts":sorts,"nulls":nulls,"expires_at":expires(report.expires_at+60),"record":encode(row)?},
    )
}
pub(super) fn row_sort_keys(row: &Row) -> (Document, Document) {
    let mut sorts = Document::new();
    let mut nulls = Document::new();
    let allocated = |value: f64| value.is_finite().then_some(value.into());
    for sort in Sort::ALL {
        let value = match sort {
            Sort::Name => Some(row.name.to_lowercase().into()),
            Sort::Owner => Some(row.owner.subject.clone().into()),
            Sort::Status => Some(row.status.clone().into()),
            Sort::LastActivityAt => row.last_activity_at.map(|v| (v as i64).into()),
            Sort::CpuMeanCores => row.measured.cpu.mean.map(Into::into),
            Sort::MemoryMeanBytes => row.measured.memory.mean.map(Into::into),
            Sort::DiskLatestBytes => row.measured.disk.latest.map(Into::into),
            Sort::AllocatedCpuCoreHours => allocated(row.allocated_period.cpu_core_hours),
            Sort::AllocatedMemoryGibHours => allocated(row.allocated_period.memory_gib_hours),
            Sort::AllocatedDiskGibHours => allocated(row.allocated_period.disk_gib_hours),
        };
        nulls.insert(sort.key(), value.is_none());
        sorts.insert(sort.key(), value.unwrap_or(mongodb::bson::Bson::Null));
    }
    (sorts, nulls)
}
