use super::*;
use crate::store::{migrations::usage_projections, transaction_scope as tx};
use cma_am_reporting::{build_chart_projections, ProjectionBatch};
use layer2_core::sandbox_metrics::Report;
use std::time::Instant;

pub(super) const COLLECTION: &str = usage_projections::COLLECTION;
pub(super) const GENERATION_FIELD: &str = "projection_generation";
const MAX_PROJECTION_BYTES: usize = 32 * 1024 * 1024;

impl ReportingStore {
    fn projections_allowed(&self, report: &Report) -> bool {
        self.view_policy.aggregate_projections_enabled
            && self.view_policy.view_scopes.iter().any(|allowed| {
                allowed.allows(&report.scope, report.query.filters.sandbox_id.as_deref())
            })
    }

    pub(crate) async fn require_projection_schema(&self) -> Result<()> {
        if self.view_policy.aggregate_projections_enabled
            && !usage_projections::inspect(&self.db)
                .await
                .map_err(unavailable)?
                .is_empty()
        {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }

    pub(super) async fn projected_generation(&self, report: &Report) -> Result<Option<i64>> {
        if !self.projections_allowed(report) {
            return Ok(None);
        }
        let job = self.initial_namespace(report).await?;
        match job.get(GENERATION_FIELD) {
            None => Ok(None),
            Some(value) => {
                let generation = job.get_i64("generation").map_err(unavailable)?;
                if value.as_i64() != Some(generation) {
                    return Err(MetricsError::Unavailable);
                }
                Ok(Some(generation))
            }
        }
    }

    pub(crate) async fn prepare_aggregate_projections(
        &self,
        lease: &ReportLease,
        report: &Report,
    ) -> Result<()> {
        if !self.projections_allowed(report) {
            return Ok(());
        }
        let started = Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            self.build_projections(lease, report),
        )
        .await;
        let batch = match result {
            Ok(Ok(batch)) => batch,
            Err(_) | Ok(Err(MetricsError::Busy)) => {
                tracing::info!(target: "cma::store::usage_reports::budget",
                    stage = "aggregate_projection", result = "generation_budget",
                    elapsed_ms = started.elapsed().as_millis() as u64);
                return Ok(());
            }
            Ok(Err(error)) => return Err(error),
        };
        let documents = batch.into_documents();
        let bytes = documents.iter().try_fold(0_i64, |total, document| {
            total
                .checked_add(document.as_bytes().len() as i64)
                .ok_or(MetricsError::Busy)
        })?;
        let stored = self.persist_projections(lease, &documents, bytes).await?;
        tracing::info!(target: "cma::store::usage_reports::budget",
            stage = "aggregate_projection", stored, bytes, chunks = documents.len(),
            elapsed_ms = started.elapsed().as_millis() as u64);
        Ok(())
    }

    async fn build_projections(
        &self,
        lease: &ReportLease,
        report: &Report,
    ) -> Result<ProjectionBatch> {
        let base = doc! {"report_id":&report.report_id,"generation":lease.generation,
        "document_schema_version":1_i64,"expires_at":expires(report.expires_at + 60)};
        let mut batch = ProjectionBatch::new(MAX_PROJECTION_BYTES);
        let deadline = Instant::now() + Duration::from_secs(10);
        build_chart_projections(
            &self.collection(CHARTS),
            report,
            &base,
            &mut batch,
            DB_DEADLINE,
            deadline,
        )
        .await?;
        if report.scope.admin {
            self.build_owner_allocation_projections(report, &base, &mut batch, deadline)
                .await?;
        }
        Ok(batch)
    }

    async fn persist_projections(
        &self,
        lease: &ReportLease,
        documents: &[mongodb::bson::RawDocumentBuf],
        bytes: i64,
    ) -> Result<bool> {
        tx::run(
            &self.client,
            (self, lease, documents, bytes),
            |&(store, lease, documents, bytes), session| {
                Box::pin(async move {
                    let fence = lease.fence();
                    let has_budget = doc! {"$and":[fence.clone(),{
                        "bytes":{"$lte":store.budgets.report_bytes - bytes},
                        "$expr":{"$lte":[{"$add":["$bytes",bytes]},"$reserved_bytes"]},
                        GENERATION_FIELD:{"$exists":false},
                    }]};
                    let write = store
                        .collection(JOBS)
                        .update_one(
                            has_budget,
                            doc! {"$inc":{"bytes":bytes},
                            "$set":{GENERATION_FIELD:lease.generation}},
                        )
                        .session(&mut *session)
                        .await
                        .map_err(tx::mongo)?;
                    if write.matched_count == 0 {
                        let current = store
                            .collection(JOBS)
                            .find_one(fence)
                            .session(&mut *session)
                            .await
                            .map_err(tx::mongo)?
                            .ok_or(MetricsError::Unavailable)?;
                        if current.contains_key(GENERATION_FIELD) {
                            return Err(MetricsError::Unavailable.into());
                        }
                        return Ok(false);
                    }
                    store
                        .collection(COLLECTION)
                        .clone_with_type::<mongodb::bson::RawDocumentBuf>()
                        .insert_many(documents)
                        .session(&mut *session)
                        .await
                        .map_err(tx::mongo)?;
                    Ok(true)
                })
            },
        )
        .await
    }
}
