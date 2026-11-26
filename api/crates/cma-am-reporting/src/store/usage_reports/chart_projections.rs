use super::charts::{fold_sandbox_series, merge_buckets, ranked_cohorts};
use super::projection_codec::encode_points;
use super::*;
use layer2_core::sandbox_metrics::{series::Bucket, Metric, Point, Report};
use mongodb::bson::RawDocumentBuf;
use sha2::{Digest, Sha256};
use std::time::Instant;

use super::ranked_buckets::RankedBuckets;
pub(super) use super::ranked_buckets::MAX_TOP;

pub(super) const POINTS_PER_CHUNK: usize = 168;

pub struct ProjectionBatch {
    documents: Vec<RawDocumentBuf>,
    remaining_bytes: usize,
}

impl ProjectionBatch {
    pub fn new(byte_budget: usize) -> Self {
        Self {
            documents: Vec::new(),
            remaining_bytes: byte_budget,
        }
    }

    pub fn push(&mut self, mut document: Document) -> Result<()> {
        let kind = document.get_str("kind").map_err(unavailable)?;
        let prefix = if kind == "contributor" { "z" } else { "p" };
        let key = doc! {
            "report_id":document.get_str("report_id").map_err(unavailable)?,
            "generation":document.get_i64("generation").map_err(unavailable)?,
            "kind":kind,
            "metric":document.get_i64("metric").map_err(unavailable)?,
            "series":document.get_str("series").map_err(unavailable)?,
            "chunk":document.get_i64("chunk").map_err(unavailable)?,
        };
        let bytes = mongodb::bson::serialize_to_vec(&key).map_err(unavailable)?;
        let id = format!("{prefix}:{:x}", Sha256::digest(bytes));
        document.insert("_id", id);
        let document = RawDocumentBuf::try_from(&document).map_err(unavailable)?;
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(document.as_bytes().len())
            .ok_or(MetricsError::Busy)?;
        self.documents.push(document);
        Ok(())
    }

    pub fn remaining_bytes(&self) -> usize {
        self.remaining_bytes
    }

    pub fn append(&mut self, other: Self) -> Result<()> {
        let used = other
            .documents
            .iter()
            .map(|row| row.as_bytes().len())
            .sum::<usize>();
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(used)
            .ok_or(MetricsError::Busy)?;
        self.documents.extend(other.documents);
        Ok(())
    }

    pub fn into_documents(mut self) -> Vec<RawDocumentBuf> {
        self.documents
            .sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        self.documents
    }
}

pub async fn build_chart_totals(
    collection: &Collection<Document>,
    report: &Report,
    base: &Document,
    batch: &mut ProjectionBatch,
    db_deadline: Duration,
    deadline: Instant,
) -> Result<()> {
    for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
        let cursor = collection
            .find(doc! {"report_id":&report.report_id,"metric":metric.index() as i64})
            .sort(doc! {"sandbox_id":1,"chunk":1})
            .batch_size(100)
            .max_time(db_deadline)
            .await
            .map_err(optional_query_error)?;
        let mut total = None;
        fold_sandbox_series(
            cursor,
            report,
            metric,
            deadline,
            optional_query_error,
            |_, _, points| {
                merge_buckets(total.get_or_insert_with(Vec::new), points);
                Ok(())
            },
        )
        .await?;
        let mut base = base.clone();
        base.insert("metric", metric.index() as i64);
        append_line(batch, &base, "chart_total", "", total)?;
    }
    Ok(())
}

pub async fn build_ranked_projections(
    collection: &Collection<Document>,
    report: &Report,
    base: &Document,
    batch: &mut ProjectionBatch,
    db_deadline: Duration,
    deadline: Instant,
) -> Result<()> {
    build_ranked_with_batch(collection, report, base, batch, db_deadline, deadline, 1000).await
}

#[cfg(feature = "test-support")]
pub async fn benchmark_ranked_projections(
    collection: &Collection<Document>,
    report: &Report,
    base: &Document,
    batch: &mut ProjectionBatch,
    deadline: Instant,
    batch_size: u32,
) -> Result<()> {
    let db_deadline = crate::pool::DB_DEADLINE;
    build_ranked_with_batch(
        collection,
        report,
        base,
        batch,
        db_deadline,
        deadline,
        batch_size,
    )
    .await
}

async fn build_ranked_with_batch(
    collection: &Collection<Document>,
    report: &Report,
    base: &Document,
    batch: &mut ProjectionBatch,
    db_deadline: Duration,
    deadline: Instant,
    batch_size: u32,
) -> Result<()> {
    // Limit fan-out to the three fixed metrics. Each task preserves Sandbox
    // order, and dropping the family deadline aborts all unfinished cursors.
    let mut tasks = tokio::task::JoinSet::new();
    for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
        tasks.spawn(fold_ranked_metric(
            collection.clone(),
            report.clone(),
            metric,
            db_deadline,
            deadline,
            batch_size,
        ));
    }
    while let Some(result) = tasks.join_next().await {
        let ranked = result.map_err(unavailable)??;
        let mut base = base.clone();
        base.insert("metric", ranked.metric.index() as i64);
        append_ranked(
            batch,
            &base,
            "chart_sandbox",
            &ranked.sandboxes,
            ranked.sandbox_buckets,
        )?;
        if report.scope.admin {
            append_ranked(
                batch,
                &base,
                "chart_owner",
                &ranked.owners,
                ranked.owner_buckets,
            )?;
        }
    }
    Ok(())
}

struct RankedMetric {
    metric: Metric,
    sandboxes: Vec<String>,
    owners: Vec<String>,
    sandbox_buckets: RankedBuckets,
    owner_buckets: RankedBuckets,
}

async fn fold_ranked_metric(
    collection: Collection<Document>,
    report: Report,
    metric: Metric,
    db_deadline: Duration,
    deadline: Instant,
    batch_size: u32,
) -> Result<RankedMetric> {
    let rank_started = Instant::now();
    let (top_sandboxes, top_owners) =
        ranked_cohorts(&collection, &report, metric, MAX_TOP, db_deadline).await?;
    let rank_ms = rank_started.elapsed().as_millis() as u64;
    tracing::info!(target: "cma::store::usage_reports::budget", stage = "ranked_projection_rank",
        metric = ?metric, batch_size, rank_ms);
    let fold_started = Instant::now();
    let mut sandbox_remaining = RankedBuckets::new(top_sandboxes.clone());
    let mut owner_remaining = RankedBuckets::new(top_owners.clone());
    let cursor = collection
        .clone_with_type::<RawDocumentBuf>()
        .find(doc! {"report_id":&report.report_id,"metric":metric.index() as i64})
        .sort(doc! {"sandbox_id":1,"chunk":1})
        .batch_size(batch_size)
        .max_time(db_deadline)
        .await
        .map_err(optional_query_error)?;
    let result = fold_sandbox_series(
        cursor,
        &report,
        metric,
        deadline,
        optional_query_error,
        |sandbox, owner, points| {
            sandbox_remaining.add_sandbox(sandbox, points)?;
            if report.scope.admin {
                owner_remaining.add_sandbox(owner, points)?;
            }
            Ok(())
        },
    )
    .await;
    tracing::info!(target: "cma::store::usage_reports::budget", stage = "ranked_projection_fold",
        metric = ?metric, batch_size, rank_ms, fold_ms = fold_started.elapsed().as_millis() as u64,
        result = ?result);
    result?;
    Ok(RankedMetric {
        metric,
        sandboxes: top_sandboxes,
        owners: top_owners,
        sandbox_buckets: sandbox_remaining,
        owner_buckets: owner_remaining,
    })
}

pub async fn build_owner_chart_projections(
    collection: &Collection<Document>,
    report: &Report,
    base: &Document,
    batch: &mut ProjectionBatch,
    db_deadline: Duration,
    deadline: Instant,
) -> Result<()> {
    for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
        let cursor = collection
            .find(doc! {"report_id":&report.report_id,"metric":metric.index() as i64})
            .sort(doc! {"owner":1,"sandbox_id":1,"chunk":1})
            .batch_size(100)
            .max_time(db_deadline)
            .await
            .map_err(optional_query_error)?;
        let mut base = base.clone();
        base.insert("metric", metric.index() as i64);
        let mut previous: Option<String> = None;
        let mut points = Vec::new();
        fold_sandbox_series(
            cursor,
            report,
            metric,
            deadline,
            optional_query_error,
            |_, owner, buckets| {
                if let Some(id) = previous.as_deref().filter(|id| *id != owner) {
                    append_line(
                        batch,
                        &base,
                        "chart_explicit_owner",
                        id,
                        Some(std::mem::take(&mut points)),
                    )?;
                }
                if previous.as_deref() != Some(owner) {
                    previous = Some(owner.to_owned());
                }
                merge_buckets(&mut points, buckets);
                Ok(())
            },
        )
        .await?;
        if let Some(id) = previous {
            append_line(batch, &base, "chart_explicit_owner", &id, Some(points))?;
        }
    }
    Ok(())
}

fn append_ranked(
    batch: &mut ProjectionBatch,
    base: &Document,
    kind: &str,
    ranks: &[String],
    remaining: RankedBuckets,
) -> Result<()> {
    let mut rank = base.clone();
    rank.extend(doc! {"kind":kind,"series":"","chunk":0_i64,"ranks":ranks});
    batch.push(rank)?;
    for (top, buckets) in remaining.into_other().into_iter().enumerate() {
        append_line(batch, base, kind, &(top + 1).to_string(), buckets)?;
    }
    Ok(())
}

pub(super) fn append_line(
    batch: &mut ProjectionBatch,
    base: &Document,
    kind: &str,
    series: &str,
    buckets: Option<Vec<Bucket>>,
) -> Result<()> {
    let present = buckets.is_some();
    let points: Vec<Point> = buckets
        .unwrap_or_default()
        .into_iter()
        .map(Bucket::point)
        .collect();
    let chunks = points.len().div_ceil(POINTS_PER_CHUNK).max(1);
    for chunk in 0..chunks {
        let start = chunk * POINTS_PER_CHUNK;
        let end = (start + POINTS_PER_CHUNK).min(points.len());
        let mut document = base.clone();
        document.extend(doc! {
            "kind": kind, "series":series,
            "chunk":chunk as i64, "chunks":chunks as i64, "present":present,
        });
        document.extend(encode_points(&points[start..end])?);
        batch.push(document)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_residual_cohort_matches_legacy_bucket_order() {
        let identities = ["ninth", "second", "other", "first", "first", "unranked"];
        let ranks = ["first", "second", "other", "ninth"];
        let buckets: Vec<_> = [
            Some(1e14),
            Some(0.25),
            None,
            Some(0.08),
            Some(0.004),
            Some(0.001),
        ]
        .into_iter()
        .map(|mean| Bucket {
            from: 10,
            to: 20,
            mean,
            covered_seconds: if mean.is_some() { 10. } else { 0. },
            expected_seconds: 10.,
            excluded_count: u64::from(mean.is_none()),
            ..Bucket::default()
        })
        .collect();

        let mut projection = RankedBuckets::new(ranks.iter().map(|id| id.to_string()).collect());
        for (id, value) in identities.iter().zip(&buckets) {
            projection
                .add_sandbox(id, std::slice::from_ref(value))
                .unwrap();
        }

        for (cutoff, cached) in projection.into_other().into_iter().enumerate() {
            let mut legacy = Vec::new();
            for (id, value) in identities.iter().zip(&buckets) {
                if *id == "other" || !ranks[..(cutoff + 1).min(ranks.len())].contains(id) {
                    merge_buckets(&mut legacy, std::slice::from_ref(value));
                }
            }
            assert_eq!(
                serde_json::to_value(cached.unwrap_or_default()).unwrap(),
                serde_json::to_value(legacy).unwrap(),
                "top={}",
                cutoff + 1
            );
        }
    }
}
