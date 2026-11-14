//! Publish a replacement only in the transaction that fences its final Ready state.
use super::*;
use layer2_core::sandbox_metrics::Report;
use views::{READ_GRACE_SECONDS, VIEW_KIND};

impl ReportingStore {
    pub(super) async fn finish_view(
        &self,
        lease: &ReportLease,
        report: &Report,
        success: bool,
    ) -> Result<()> {
        let view = lease.view.as_ref().ok_or(MetricsError::Unavailable)?;
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        session.start_transaction().await.map_err(unavailable)?;
        let job = self
            .collection(JOBS)
            .find_one(lease.fence())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::Unavailable)?;
        let at = now();
        if success {
            let expected = report
                .summary
                .as_ref()
                .ok_or(MetricsError::Unavailable)?
                .sandbox_count;
            let rows = self
                .collection(ROWS)
                .count_documents(doc! {"report_id":&report.report_id})
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            let charts = self
                .collection(CHARTS)
                .count_documents(doc! {"report_id":&report.report_id})
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            if expected != rows
                || i64::try_from(rows).ok() != job.get_i64("row_count").ok()
                || i64::try_from(charts).ok() != job.get_i64("chart_count").ok()
                || charts < rows.saturating_mul(3)
            {
                return Err(MetricsError::Unavailable);
            }
        }
        if report.expires_at <= at || report.expires_at != lease.report.expires_at {
            return Err(MetricsError::Expired);
        }
        let pointer_filter = doc! {"_id":&view.key,"kind":VIEW_KIND,"revision":view.revision,"active_id":&report.report_id};
        let pointer = self
            .collection(CONTROL)
            .find_one(pointer_filter.clone())
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::Unavailable)?;
        let state = if success { "view_ready" } else { "view_failed" };
        let updated = self
            .collection(JOBS)
            .update_one(
                lease.fence(),
                vec![doc! {"$set":{
                    "record":{"$literal":encode(report)?},"state":state,"lease_until":0_i64,
                    "reserved_bytes":"$bytes","generated_at":at as i64,
                "retire_after":if success { report.expires_at as i64 } else { at as i64 }}}],
            )
            .session(&mut session)
            .await
            .map_err(unavailable)?;
        if updated.matched_count != 1 {
            return Err(MetricsError::Unavailable);
        }
        let change = if success {
            doc! {"$set":{"latest_id":&report.report_id,"latest_generation":lease.generation,
            "latest_retain_until":report.expires_at as i64,"generated_at":at as i64},"$unset":{"active_id":""}}
        } else {
            doc! {"$unset":{"active_id":""}}
        };
        if self
            .collection(CONTROL)
            .update_one(pointer_filter, change)
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .matched_count
            != 1
        {
            return Err(MetricsError::Unavailable);
        }
        if success {
            if let Ok(previous) = pointer.get_str("latest_id") {
                self.collection(JOBS)
                    .update_one(
                        doc! {"_id":previous,"kind":VIEW_KIND,"state":"view_ready"},
                        doc! {"$set":{"retire_after":(at+READ_GRACE_SECONDS) as i64}},
                    )
                    .session(&mut session)
                    .await
                    .map_err(unavailable)?;
            }
        }
        if let Err(error) = session.commit_transaction().await {
            // Reconcile an ambiguous commit by exact receipts, never a new job.
            let completed = self.collection(JOBS).find_one(doc! {"_id":&report.report_id,"generation":lease.generation,"state":state,"lease_until":0_i64})
                .max_time(DB_DEADLINE).await.map_err(unavailable)?;
            let pointer = self
                .collection(CONTROL)
                .find_one(doc! {"_id":&view.key,"revision":view.revision})
                .max_time(DB_DEADLINE)
                .await
                .map_err(unavailable)?;
            let committed = completed.is_some()
                && pointer.as_ref().is_some_and(|p| {
                    !p.contains_key("active_id")
                        && (!success
                            || (p.get_str("latest_id").ok() == Some(&report.report_id)
                                && p.get_i64("latest_generation").ok() == Some(lease.generation)))
                });
            if !committed {
                return Err(unavailable(error));
            }
        }
        self.release(&lease.slot, &lease.holder).await?;
        self.notifications.finished(&report.report_id);
        Ok(())
    }
}
