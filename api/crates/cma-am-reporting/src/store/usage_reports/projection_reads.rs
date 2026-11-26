use super::chart_projections::{MAX_TOP, POINTS_PER_CHUNK};
use super::charts::{fold_sandbox_series, merge_buckets, read_series};
use super::projection_codec::decode_points;
use super::projection_manifest::{document_digest, ProjectionManifest};
use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    series::Bucket, GroupBy, Line, Point, Report, Series, SeriesQuery, SOURCE,
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Instant};

pub async fn read_chart_projection(
    projections: &Collection<Document>,
    charts: Collection<Document>,
    report: &Report,
    query: SeriesQuery,
    manifest: &ProjectionManifest,
    db_deadline: Duration,
) -> Result<Series> {
    let top = query.validate(report.scope.admin)?;
    if query.group_by == GroupBy::Sandbox && query.series_id.is_some() {
        return read_series(charts, report, query, db_deadline).await;
    }
    let base = doc! {"report_id":&report.report_id,"generation":manifest.generation,
    "document_schema_version":1_i64,"metric":query.metric.index() as i64};
    let mut lines = Vec::new();
    match (query.group_by, query.series_id.as_deref()) {
        (GroupBy::Total, _) => {
            let points = read_line(
                projections,
                &base,
                manifest,
                report,
                "chart_total",
                "",
                db_deadline,
            )
            .await?
            .ok_or(MetricsError::Unavailable)?;
            if let Some(points) = points {
                lines.push(line("total", points));
            }
        }
        (GroupBy::Owner, Some(owner)) => {
            let points = read_owner_line(
                projections,
                &charts,
                &base,
                manifest,
                report,
                owner,
                db_deadline,
            )
            .await?;
            lines.push(line(owner, points));
        }
        (group, None) => {
            let kind = match group {
                GroupBy::Owner => "chart_owner",
                GroupBy::Sandbox => "chart_sandbox",
                GroupBy::Total => unreachable!(),
            };
            let rank = projections
                .find_one(projection_filter(&base, kind, ""))
                .max_time(db_deadline)
                .await
                .map_err(unavailable)?
                .ok_or(MetricsError::Unavailable)?;
            manifest.verify_document(kind, query.metric.index() as i64, "", &rank)?;
            let ranks: Vec<String> = mongodb::bson::deserialize_from_bson(
                rank.get("ranks")
                    .cloned()
                    .ok_or(MetricsError::Unavailable)?,
            )
            .map_err(unavailable)?;
            if ranks.len() > MAX_TOP
                || ranks.iter().collect::<std::collections::HashSet<_>>().len() != ranks.len()
            {
                return Err(MetricsError::Unavailable);
            }
            let ids: Vec<_> = ranks
                .into_iter()
                .take(top)
                .filter(|id| id != "other")
                .collect();
            let selected = if group == GroupBy::Owner {
                if manifest.has_family("chart_explicit_owner") {
                    read_selected_owners(projections, &base, manifest, report, &ids, db_deadline)
                        .await?
                } else {
                    read_ranked_owner_sources(&charts, report, &query, &ids, db_deadline).await?
                }
            } else {
                read_selected_sandboxes(&charts, report, &query, &ids, db_deadline).await?
            };
            lines.extend(selected);
            let other = read_line(
                projections,
                &base,
                manifest,
                report,
                kind,
                &top.to_string(),
                db_deadline,
            )
            .await?
            .ok_or(MetricsError::Unavailable)?;
            if let Some(points) = other {
                lines.push(line("other", points));
            }
            lines.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        }
        (GroupBy::Sandbox, Some(_)) => unreachable!(),
    }
    Ok(Series {
        report_id: report.report_id.clone(),
        metric: query.metric,
        unit: query.metric.unit().into(),
        source: SOURCE.into(),
        lines,
    })
}

async fn read_owner_line(
    projections: &Collection<Document>,
    charts: &Collection<Document>,
    base: &Document,
    manifest: &ProjectionManifest,
    report: &Report,
    owner: &str,
    db_deadline: Duration,
) -> Result<Vec<Point>> {
    if let Some(points) = read_line(
        projections,
        base,
        manifest,
        report,
        "chart_explicit_owner",
        owner,
        db_deadline,
    )
    .await?
    {
        return points.ok_or(MetricsError::Unavailable);
    }
    let source = charts
        .find_one(
            doc! {"report_id":base.get_str("report_id").map_err(unavailable)?,
            "metric":base.get_i64("metric").map_err(unavailable)?,"owner":owner},
        )
        .projection(doc! {"_id":1})
        .max_time(db_deadline)
        .await
        .map_err(unavailable)?;
    if source.is_some() {
        Err(MetricsError::Unavailable)
    } else {
        Err(MetricsError::NotFound)
    }
}

fn projection_filter(base: &Document, kind: &str, series: &str) -> Document {
    let mut filter = base.clone();
    filter.extend(doc! {"kind":kind,"series":series});
    filter
}

// The public report range is capped at 2,016 buckets.
const MAX_CHUNKS: usize = 2016 / POINTS_PER_CHUNK;

#[derive(Default)]
struct ProjectionChunks {
    points: Vec<Point>,
    expected: Option<(usize, bool)>,
    count: usize,
    digest: Sha256,
}
impl ProjectionChunks {
    fn push(&mut self, document: Document) -> Result<()> {
        let chunks = document.get_i64("chunks").map_err(unavailable)? as usize;
        let present = document.get_bool("present").map_err(unavailable)?;
        let index = document.get_i64("chunk").map_err(unavailable)? as usize;
        if !(1..=MAX_CHUNKS).contains(&chunks)
            || index != self.count
            || index >= chunks
            || self.expected.is_some_and(|old| old != (chunks, present))
        {
            return Err(MetricsError::Unavailable);
        }
        self.expected = Some((chunks, present));
        let part = decode_points(&document)?;
        if part.len() > POINTS_PER_CHUNK
            || (index + 1 < chunks && part.len() != POINTS_PER_CHUNK)
            || (!present && (chunks != 1 || !part.is_empty()))
        {
            return Err(MetricsError::Unavailable);
        }
        self.digest.update(document_digest(&document)?);
        self.points.extend(part);
        self.count += 1;
        Ok(())
    }

    fn finish(
        self,
        manifest: &ProjectionManifest,
        metric: i64,
        report: &Report,
        kind: &str,
        series: &str,
    ) -> Result<Option<Option<Vec<Point>>>> {
        let verified = manifest.verify_line(kind, metric, series, self.count, self.digest)?;
        match self.expected {
            Some((chunks, present)) if verified && chunks == self.count => {
                if present {
                    let range = report.effective_range;
                    let step = report.query.step_seconds;
                    let mut at = range.from / step * step;
                    for point in &self.points {
                        if at >= range.to
                            || point.from != at.max(range.from)
                            || point.to != (at + step).min(range.to)
                        {
                            return Err(MetricsError::Unavailable);
                        }
                        at += step;
                    }
                    if at < range.to && range.seconds() > 0 {
                        return Err(MetricsError::Unavailable);
                    }
                }
                Ok(Some(present.then_some(self.points)))
            }
            None if !verified => Ok(None),
            _ => Err(MetricsError::Unavailable),
        }
    }
}

async fn read_line(
    collection: &Collection<Document>,
    base: &Document,
    manifest: &ProjectionManifest,
    report: &Report,
    kind: &str,
    series: &str,
    db_deadline: Duration,
) -> Result<Option<Option<Vec<Point>>>> {
    let mut cursor = collection
        .find(projection_filter(base, kind, series))
        .sort(doc! {"chunk":1})
        .limit(MAX_CHUNKS as i64 + 1)
        .max_time(db_deadline)
        .await
        .map_err(unavailable)?;
    let mut chunks = ProjectionChunks::default();
    while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
        chunks.push(document)?;
    }
    chunks.finish(
        manifest,
        base.get_i64("metric").map_err(unavailable)?,
        report,
        kind,
        series,
    )
}

async fn read_selected_owners(
    projections: &Collection<Document>,
    base: &Document,
    manifest: &ProjectionManifest,
    report: &Report,
    ids: &[String],
    db_deadline: Duration,
) -> Result<Vec<Line>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut filter = base.clone();
    filter.extend(doc! {"kind":"chart_explicit_owner", "series":{"$in":ids}});
    let mut cursor = projections
        .find(filter)
        .sort(doc! {"series":1,"chunk":1})
        .limit((ids.len() * MAX_CHUNKS + 1) as i64)
        .max_time(db_deadline)
        .await
        .map_err(unavailable)?;
    let mut lines = BTreeMap::<String, ProjectionChunks>::new();
    while let Some(row) = cursor.try_next().await.map_err(unavailable)? {
        let id = row.get_str("series").map_err(unavailable)?;
        if !ids.iter().any(|expected| expected == id) {
            return Err(MetricsError::Unavailable);
        }
        lines.entry(id.into()).or_default().push(row)?;
    }
    if lines.len() != ids.len() {
        return Err(MetricsError::Unavailable);
    }
    lines
        .into_iter()
        .map(|(id, chunks)| {
            let points = chunks
                .finish(
                    manifest,
                    base.get_i64("metric").map_err(unavailable)?,
                    report,
                    "chart_explicit_owner",
                    &id,
                )?
                .flatten()
                .ok_or(MetricsError::Unavailable)?;
            Ok(line(&id, points))
        })
        .collect()
}

async fn read_ranked_owner_sources(
    charts: &Collection<Document>,
    report: &Report,
    query: &SeriesQuery,
    ids: &[String],
    db_deadline: Duration,
) -> Result<Vec<Line>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let cursor = charts
        .find(
            doc! {"report_id":&report.report_id,"metric":query.metric.index() as i64,
            "owner":{"$in":ids}},
        )
        .sort(doc! {"owner":1,"sandbox_id":1,"chunk":1})
        .max_time(db_deadline)
        .await
        .map_err(unavailable)?;
    let mut grouped = BTreeMap::<String, Vec<Bucket>>::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    fold_sandbox_series(
        cursor,
        report,
        query.metric,
        deadline,
        unavailable,
        |_, owner, points| {
            if !ids.iter().any(|id| id == owner) {
                return Err(MetricsError::Unavailable);
            }
            merge_buckets(grouped.entry(owner.into()).or_default(), points);
            Ok(())
        },
    )
    .await?;
    if grouped.len() != ids.len() {
        return Err(MetricsError::Unavailable);
    }
    Ok(grouped
        .into_iter()
        .map(|(id, buckets)| line(&id, buckets.into_iter().map(Bucket::point).collect()))
        .collect())
}

async fn read_selected_sandboxes(
    charts: &Collection<Document>,
    report: &Report,
    query: &SeriesQuery,
    ids: &[String],
    db_deadline: Duration,
) -> Result<Vec<Line>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let cursor = charts
        .find(
            doc! {"report_id":&report.report_id,"metric":query.metric.index() as i64,
            "sandbox_id":{"$in":ids}},
        )
        .sort(doc! {"sandbox_id":1,"chunk":1})
        .limit((ids.len() * MAX_CHUNKS + 1) as i64)
        .max_time(db_deadline)
        .await
        .map_err(unavailable)?;
    let mut lines = BTreeMap::<String, Vec<Point>>::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    fold_sandbox_series(
        cursor,
        report,
        query.metric,
        deadline,
        unavailable,
        |sandbox, _, buckets| {
            if !ids.iter().any(|id| id == sandbox) {
                return Err(MetricsError::Unavailable);
            }
            let points = buckets.iter().cloned().map(Bucket::point).collect();
            if lines.insert(sandbox.into(), points).is_some() {
                return Err(MetricsError::Unavailable);
            }
            Ok(())
        },
    )
    .await?;
    if lines.len() != ids.len() {
        return Err(MetricsError::Unavailable);
    }
    Ok(lines
        .into_iter()
        .map(|(id, points)| line(&id, points))
        .collect())
}

fn line(id: &str, points: Vec<Point>) -> Line {
    let label = match id {
        "total" => "Total",
        "other" => "Other",
        _ => id,
    };
    Line {
        id: id.into(),
        label: label.into(),
        points,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::usage_reports::chart_projections::{append_line, ProjectionBatch};
    use layer2_core::sandbox_metrics::{Range, ReportQuery, ReportStatus, Scope};

    #[test]
    fn projected_line_requires_complete_frozen_time_extent() {
        let report = Report {
            schema_version: 1,
            report_id: "fixture".into(),
            status: ReportStatus::Ready,
            report_complete: true,
            scope: Scope {
                principal: "fixture-owner".into(),
                admin: true,
            },
            query: ReportQuery {
                from: 0,
                to: 120,
                step_seconds: 60,
                filters: Default::default(),
            },
            as_of: 180,
            measurement_cutoff: 120,
            effective_range: Range { from: 0, to: 120 },
            expires_at: 300,
            allocation_watermark: String::new(),
            mapping_revision: 0,
            available_from: None,
            tracking_started_at: None,
            warnings: Vec::new(),
            source_health: "complete".into(),
            summary: None,
            poll_url: String::new(),
            retry_after_seconds: 0,
        };
        let buckets = [
            Bucket {
                from: 0,
                to: 60,
                ..Default::default()
            },
            Bucket {
                from: 60,
                to: 120,
                ..Default::default()
            },
        ];
        let check = |buckets: &[Bucket]| {
            let mut batch = ProjectionBatch::new(32_768);
            append_line(&mut batch, &doc! {"report_id":"fixture","generation":1_i64,"metric":0_i64,"document_schema_version":1_i64}, "chart_total", "", Some(buckets.to_vec())).unwrap();
            let documents = batch.into_documents();
            let bytes = documents
                .iter()
                .map(|document| document.as_bytes().len() as i64)
                .sum();
            let manifest = ProjectionManifest::build(1, &documents, bytes).unwrap();
            let manifest = ProjectionManifest::parse(1, &manifest).unwrap();
            let mut chunks = ProjectionChunks::default();
            for document in documents {
                chunks.push(Document::try_from(document).unwrap()).unwrap();
            }
            chunks.finish(&manifest, 0, &report, "chart_total", "")
        };
        assert_eq!(check(&buckets).unwrap().unwrap().unwrap().len(), 2);
        assert!(matches!(
            check(&buckets[..1]),
            Err(MetricsError::Unavailable)
        ));
    }
}
