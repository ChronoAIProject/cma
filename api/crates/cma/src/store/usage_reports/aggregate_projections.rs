use super::*;
use crate::store::{migrations::usage_projections, transaction_scope as tx};
use cma_am_reporting::{
    build_chart_totals, build_owner_chart_projections, build_ranked_projections, ProjectionBatch,
    ProjectionManifest, TotalProjection,
};
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::Report;
use mongodb::bson::RawDocumentBuf;
use std::time::Instant;

pub(super) const COLLECTION: &str = usage_projections::COLLECTION;
pub(super) const GENERATION_FIELD: &str = "projection_generation";
pub(super) const MANIFEST_FIELD: &str = "projection_manifest";
pub(in crate::store) const INVALIDATED_FIELD: &str = "projection_invalidated";
const MAX_PROJECTION_BYTES: usize = 32 * 1024 * 1024;

impl ReportingStore {
    pub(crate) fn projections_allowed(&self, report: &Report) -> bool {
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

    pub(crate) async fn prepare_aggregate_projections(
        &self,
        lease: &ReportLease,
        report: &Report,
        totals: Option<TotalProjection>,
        overall_deadline: Instant,
    ) -> Result<()> {
        if !self.projections_allowed(report) {
            return Ok(());
        }
        let started = Instant::now();
        let (batch, row_count, chart_count) = match self
            .build_projections_with_totals(lease, report, totals, overall_deadline)
            .await
        {
            Ok(batch) => batch,
            Err(MetricsError::Busy) => {
                tracing::info!(target: "cma::store::usage_reports::budget",
                    stage = "aggregate_projection", result = "generation_budget",
                    elapsed_ms = started.elapsed().as_millis() as u64);
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let remaining = batch.remaining_bytes();
        let mut documents = batch.into_documents();
        let budget = remaining.saturating_add(
            documents
                .iter()
                .map(|document| document.as_bytes().len())
                .sum::<usize>(),
        );
        // Optional result families may be omitted as a whole if their parent
        // commitments do not fit the exact reserved bytes or metadata limit.
        let mut optional = [
            "chart_explicit_owner",
            "owner_allocation",
            "chart_sandbox",
            "chart_owner",
        ]
        .into_iter();
        let (manifest, bytes) = loop {
            let page_bytes = documents.iter().try_fold(0_i64, |total, document| {
                total
                    .checked_add(document.as_bytes().len() as i64)
                    .ok_or(MetricsError::Busy)
            })?;
            match Self::projection_manifest(lease, &documents, page_bytes) {
                Ok((manifest, charged))
                    if usize::try_from(charged).is_ok_and(|charged| charged <= budget) =>
                {
                    break (manifest, charged)
                }
                Ok(_) | Err(MetricsError::Busy) => {
                    let Some(kind) = optional.next() else {
                        return Ok(());
                    };
                    documents.retain(|document| document.get_str("kind").ok() != Some(kind));
                    tracing::info!(target: "cma::store::usage_reports::budget", stage="aggregate_projection", family=kind, result="manifest_budget");
                }
                Err(error) => return Err(error),
            }
        };
        if documents.is_empty() {
            return Ok(());
        }
        let stored = self
            .persist_projection_manifest(
                lease,
                &documents,
                bytes,
                &manifest,
                (row_count, chart_count),
            )
            .await?;
        tracing::info!(target: "cma::store::usage_reports::budget",
            stage = "aggregate_projection", stored, bytes, chunks = documents.len(),
            elapsed_ms = started.elapsed().as_millis() as u64);
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn build_projections(
        &self,
        lease: &ReportLease,
        report: &Report,
    ) -> Result<(ProjectionBatch, i64, i64)> {
        self.build_projections_with_totals(
            lease,
            report,
            None,
            Instant::now() + Duration::from_secs(30),
        )
        .await
    }

    async fn build_projections_with_totals(
        &self,
        lease: &ReportLease,
        report: &Report,
        totals: Option<TotalProjection>,
        overall_deadline: Instant,
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
        // All completed families share the original report byte reservation.
        // Each optional family is built in a scratch batch. Only complete
        // families enter the immutable generation manifest.
        let work_deadline = overall_deadline
            .checked_sub(Duration::from_secs(3))
            .ok_or(MetricsError::Busy)?;
        let budget = self
            .budgets
            .report_bytes
            .min(job.get_i64("reserved_bytes").map_err(unavailable)?)
            .saturating_sub(job.get_i64("bytes").map_err(unavailable)?);
        let budget = usize::try_from(budget)
            .map_err(|_| MetricsError::Busy)?
            .min(MAX_PROJECTION_BYTES);
        let mut batch = ProjectionBatch::new(budget);
        let deadline = work_deadline.min(Instant::now() + Duration::from_secs(10));
        if Instant::now() >= deadline {
            return Err(MetricsError::Busy);
        }
        Self::within_family_deadline(
            deadline,
            self.build_contributors(report, &base, &mut batch, deadline),
        )
        .await?;
        let mut family = ProjectionBatch::new(batch.remaining_bytes());
        if let Some(totals) = totals {
            totals.append(&base, &mut family, row_count, &lease.holder)?;
        } else {
            Self::within_family_deadline(
                deadline,
                build_chart_totals(
                    &self.collection(CHARTS),
                    report,
                    &base,
                    &mut family,
                    DB_DEADLINE,
                    deadline,
                ),
            )
            .await?;
        }
        batch.append(family)?;

        // Optional construction can time out or refuse its remaining bytes.
        // A scratch batch never contributes an incomplete family.
        let charts = self.collection(CHARTS);
        let mut family = ProjectionBatch::new(batch.remaining_bytes());
        // Three metric folds share the remaining report deadline. A ranked
        // family publishes only after all metrics finish within this bound.
        let deadline = work_deadline.min(Instant::now() + Duration::from_secs(20));
        if Instant::now() < deadline {
            let result = Self::within_family_deadline(
                deadline,
                build_ranked_projections(
                    &charts,
                    report,
                    &base,
                    &mut family,
                    DB_DEADLINE,
                    deadline,
                ),
            )
            .await;
            Self::finish_family(&mut batch, family, result, "chart_ranked")?;
        }
        let mut family = ProjectionBatch::new(batch.remaining_bytes());
        let deadline = work_deadline.min(Instant::now() + Duration::from_secs(2));
        if Instant::now() < deadline && report.scope.admin {
            let result = Self::within_family_deadline(
                deadline,
                self.build_owner_allocation_projections(report, &base, &mut family, deadline),
            )
            .await;
            Self::finish_family(&mut batch, family, result, "owner_allocation")?;
        }
        let mut family = ProjectionBatch::new(batch.remaining_bytes());
        let deadline = work_deadline.min(Instant::now() + Duration::from_secs(10));
        if Instant::now() < deadline && report.scope.admin {
            let result = Self::within_family_deadline(
                deadline,
                build_owner_chart_projections(
                    &charts,
                    report,
                    &base,
                    &mut family,
                    DB_DEADLINE,
                    deadline,
                ),
            )
            .await;
            Self::finish_family(&mut batch, family, result, "chart_explicit_owner")?;
        }
        Ok((batch, row_count, chart_count))
    }

    async fn within_family_deadline<T>(
        deadline: Instant,
        work: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), work)
            .await
            .map_err(|_| MetricsError::Busy)?
    }

    fn finish_family(
        batch: &mut ProjectionBatch,
        family: ProjectionBatch,
        result: Result<()>,
        name: &'static str,
    ) -> Result<()> {
        match result {
            Ok(()) => batch.append(family),
            Err(MetricsError::Busy) => {
                tracing::info!(target: "cma::store::usage_reports::budget", stage = "aggregate_projection", family = name, result = "family_budget");
                Ok(())
            }
            Err(error) => Err(error),
        }
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
            .map_err(cma_am_reporting::optional_query_error)?;
        while let Some(row) = cursor
            .try_next()
            .await
            .map_err(cma_am_reporting::optional_query_error)?
        {
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
        page_bytes: i64,
    ) -> Result<(Document, i64)> {
        let mut manifest = ProjectionManifest::build(lease.generation, documents, page_bytes)?;
        ProjectionManifest::bounded_len(&manifest)?;
        // BSON embeds the same elements without a second document header.
        let increment = mongodb::bson::serialize_to_vec(
            &doc! {GENERATION_FIELD: lease.generation, MANIFEST_FIELD: &manifest},
        )
        .map_err(unavailable)?
        .len()
            - 5;
        let charged = page_bytes
            .checked_add(i64::try_from(increment).map_err(unavailable)?)
            .ok_or(MetricsError::Busy)?;
        manifest.insert("bytes", charged);
        Ok((manifest, charged))
    }

    #[cfg(test)]
    pub(super) async fn persist_projections(
        &self,
        lease: &ReportLease,
        documents: &[RawDocumentBuf],
        bytes: i64,
        source_counts: (i64, i64),
    ) -> Result<bool> {
        let (manifest, bytes) = match Self::projection_manifest(lease, documents, bytes) {
            Ok(value) => value,
            Err(MetricsError::Busy) => return Ok(false),
            Err(error) => return Err(error),
        };
        self.persist_projection_manifest(lease, documents, bytes, &manifest, source_counts)
            .await
    }

    async fn persist_projection_manifest(
        &self,
        lease: &ReportLease,
        documents: &[RawDocumentBuf],
        bytes: i64,
        manifest: &Document,
        source_counts: (i64, i64),
    ) -> Result<bool> {
        let result = tx::run(
            &self.client,
            (self, lease, documents, bytes, manifest, source_counts),
            |&(store, lease, documents, bytes, manifest, (rows, charts)), session| {
                Box::pin(async move {
                    let mut fence = lease.fence();
                    fence.insert(INVALIDATED_FIELD, doc! {"$exists":false});
                    fence.insert("row_count", rows);
                    fence.insert("chart_count", charts);
                    let parent = store
                        .collection(JOBS)
                        .find_one(fence.clone())
                        .session(&mut *session)
                        .await
                        .map_err(tx::mongo)?
                        .ok_or(MetricsError::Unavailable)?;
                    let mut sized = parent;
                    sized.insert(GENERATION_FIELD, lease.generation);
                    sized.insert(MANIFEST_FIELD, manifest.clone());
                    if mongodb::bson::serialize_to_vec(&sized)
                        .map_err(tx::unavailable)?
                        .len()
                        > 15 * 1024 * 1024
                    {
                        return Ok(false);
                    }
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
                && job.get_document(MANIFEST_FIELD).ok() == Some(manifest)
        }) {
            return Ok(true);
        }
        result
    }
}
