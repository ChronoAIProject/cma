use super::*;
use crate::store::{migrations::usage_projections, transaction_scope as tx};
use cma_am_reporting::{build_chart_projections, ProjectionBatch};
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::Report;
use mongodb::bson::RawDocumentBuf;
use sha2::{Digest, Sha256};
use std::time::Instant;

pub(super) const COLLECTION: &str = usage_projections::COLLECTION;
pub(super) const GENERATION_FIELD: &str = "projection_generation";
pub(super) const MANIFEST_FIELD: &str = "projection_manifest";
pub(in crate::store) const INVALIDATED_FIELD: &str = "projection_invalidated";
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
                if value.as_i64() != Some(generation)
                    || job.get_document(MANIFEST_FIELD).is_err()
                    || job.contains_key(INVALIDATED_FIELD)
                {
                    return Err(MetricsError::Unavailable);
                }
                if !job.contains_key("kind") {
                    // Ordinary reports also need full source membership once
                    // independently persisted aggregates can outlive a row.
                    self.retained_counts(report, &job).await?;
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
        let (batch, row_count, chart_count) = match result {
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
        let stored = self
            .persist_projections(lease, &documents, bytes, (row_count, chart_count))
            .await?;
        tracing::info!(target: "cma::store::usage_reports::budget",
            stage = "aggregate_projection", stored, bytes, chunks = documents.len(),
            elapsed_ms = started.elapsed().as_millis() as u64);
        Ok(())
    }

    pub(super) async fn build_projections(
        &self,
        lease: &ReportLease,
        report: &Report,
    ) -> Result<(ProjectionBatch, i64, i64)> {
        // The job counts are written together with every immutable source chunk.
        // Erasure fences this parent before removing any source document.
        let job = self
            .collection(JOBS)
            .find_one(lease.fence())
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::Unavailable)?;
        if job.contains_key(INVALIDATED_FIELD) {
            return Err(MetricsError::Unavailable);
        }
        let row_count = job.get_i64("row_count").map_err(unavailable)?;
        let chart_count = job.get_i64("chart_count").map_err(unavailable)?;
        self.retained_counts(report, &job).await?;
        let base = doc! {"report_id":&report.report_id,"generation":lease.generation,
        "document_schema_version":1_i64,"expires_at":expires(report.expires_at + 60)};
        let mut batch = ProjectionBatch::new(MAX_PROJECTION_BYTES);
        let deadline = Instant::now() + Duration::from_secs(10);
        self.build_contributors(report, &base, &mut batch, deadline)
            .await?;
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
        Ok((batch, row_count, chart_count))
    }

    async fn build_contributors(
        &self,
        report: &Report,
        base: &Document,
        batch: &mut ProjectionBatch,
        deadline: Instant,
    ) -> Result<()> {
        let mut cursor = self
            .collection(ROWS)
            .aggregate([
                doc! {"$match":{"report_id":&report.report_id}},
                doc! {"$group":{"_id":"$owner"}},
            ])
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
            if Instant::now() >= deadline {
                return Err(MetricsError::Busy);
            }
            let owner = row.get_str("_id").map_err(unavailable)?;
            let mut membership = base.clone();
            // Evidence of contribution must survive independently scheduled TTL
            // deletion of source rows and result pages.
            membership.remove("expires_at");
            membership.extend(
                doc! {"kind":"contributor","series":owner,"contributor":owner,
                "metric":0_i64,"chunk":0_i64},
            );
            batch.push(membership)?;
        }
        Ok(())
    }

    fn projection_manifest(
        lease: &ReportLease,
        documents: &[RawDocumentBuf],
        bytes: i64,
    ) -> Document {
        let mut digest = Sha256::new();
        for document in documents {
            digest.update(document.as_bytes());
        }
        doc! {"version":1_i64,"generation":lease.generation,"chunks":documents.len() as i64,
        "bytes":bytes,"sha256":format!("{:x}",digest.finalize())}
    }

    pub(super) async fn persist_projections(
        &self,
        lease: &ReportLease,
        documents: &[RawDocumentBuf],
        bytes: i64,
        source_counts: (i64, i64),
    ) -> Result<bool> {
        let manifest = Self::projection_manifest(lease, documents, bytes);
        let result = tx::run(
            &self.client,
            (self, lease, documents, bytes, &manifest, source_counts),
            |&(store, lease, documents, bytes, manifest, (rows, charts)), session| {
                Box::pin(async move {
                    let mut fence = lease.fence();
                    fence.insert(INVALIDATED_FIELD, doc! {"$exists":false});
                    fence.insert("row_count", rows);
                    fence.insert("chart_count", charts);
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
                            "$set":{GENERATION_FIELD:lease.generation,MANIFEST_FIELD:manifest,
                                "expires_at":mongodb::bson::Bson::Null}},
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
                        return match current.get(GENERATION_FIELD) {
                            None => Ok(false),
                            Some(value)
                                if value.as_i64() == Some(lease.generation)
                                    && current.get_document(MANIFEST_FIELD).ok()
                                        == Some(manifest) =>
                            {
                                Ok(true)
                            }
                            Some(_) => Err(MetricsError::Unavailable.into()),
                        };
                    }
                    for (collection, expected) in [(ROWS, rows), (CHARTS, charts)] {
                        let actual = store
                            .collection(collection)
                            .count_documents(doc! {"report_id":&lease.report.report_id})
                            .session(&mut *session)
                            .await
                            .map_err(tx::mongo)?;
                        if i64::try_from(actual).ok() != Some(expected) {
                            return Err(MetricsError::Unavailable.into());
                        }
                    }
                    store
                        .collection(COLLECTION)
                        .clone_with_type::<RawDocumentBuf>()
                        .insert_many(documents)
                        .session(&mut *session)
                        .await
                        .map_err(tx::mongo)?;
                    Ok(true)
                })
            },
        )
        .await;
        if !matches!(result, Err(MetricsError::Unavailable)) {
            return result;
        }
        // An acknowledged marker and its pages are one commit. Read the exact
        // manifest from the original fenced job before accepting an uncertain reply.
        let mut fence = lease.fence();
        fence.insert(INVALIDATED_FIELD, doc! {"$exists":false});
        fence.insert("row_count", source_counts.0);
        fence.insert("chart_count", source_counts.1);
        let current = crate::store::durability::majority_collection::<Document>(&self.db, JOBS)
            .find_one(fence)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        if current.as_ref().is_some_and(|job| {
            job.get_i64(GENERATION_FIELD).ok() == Some(lease.generation)
                && job.get_document(MANIFEST_FIELD).ok() == Some(&manifest)
        }) {
            return Ok(true);
        }
        result
    }
}
