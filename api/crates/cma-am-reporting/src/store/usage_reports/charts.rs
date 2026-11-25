use super::chart_encoding::{ChartDocument, ChartRecord};
use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    allocation::Expectation,
    series::{self, Bucket},
    GroupBy, Line, Metric, Report, Series, SeriesQuery,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChartChunk {
    buckets: Vec<Bucket>,
    expectations: Vec<Expectation>,
}
pub fn chart_documents(
    report: &Report,
    sandbox: &str,
    owner: &str,
    metric: Metric,
    buckets: Vec<Bucket>,
    expectations: &[Expectation],
) -> Result<Vec<mongodb::bson::RawDocumentBuf>> {
    let rank: f64 = buckets.iter().map(|b| b.integral).sum();
    let expectations = metric_expectations(expectations, metric);
    let mut out = Vec::new();
    // A membership marker is required even if all measurements are absent.
    // Its compact intervals reconstruct null buckets without caching zeros.
    for chunk in 0..buckets.len().div_ceil(168).max(1) {
        let points =
            &buckets[(chunk * 168).min(buckets.len())..((chunk + 1) * 168).min(buckets.len())];
        out.push(
            ChartDocument {
                id: format!("{}:{sandbox}:{}:{chunk}", report.report_id, metric.index()),
                document_schema_version: 1,
                report_id: &report.report_id,
                sandbox_id: sandbox,
                owner,
                group: "sandbox",
                series_id: sandbox,
                metric: metric.index() as i64,
                chunk: chunk as i64,
                rank,
                expires_at: expires(report.expires_at + 60),
                record: ChartRecord {
                    buckets: points,
                    expectations: if chunk == 0 { &expectations } else { &[] },
                },
            }
            .encode()?,
        );
    }
    Ok(out)
}

pub(super) fn metric_expectations(
    expectations: &[Expectation],
    metric: Metric,
) -> Vec<Expectation> {
    let value = |e: &Expectation| {
        if metric == Metric::Disk {
            e.home
        } else {
            e.running
        }
    };
    let mut out: Vec<Expectation> = Vec::with_capacity(expectations.len());
    for next in expectations {
        if let Some(last) = out.last_mut() {
            // Chart readers use only this metric's expectation. Preserve gaps,
            // overlaps and unknown intervals; merge only adjacent equal states.
            if last.from < last.to
                && next.from < next.to
                && last.to == next.from
                && value(last) == value(next)
            {
                last.to = next.to;
                if last.running != next.running {
                    last.running = None;
                }
                if last.home != next.home {
                    last.home = None;
                }
                continue;
            }
        }
        out.push(next.clone());
    }
    out
}

pub async fn read_series(
    collection: Collection<Document>,
    report: &Report,
    query: SeriesQuery,
    db_deadline: Duration,
) -> Result<Series> {
    read_series_with_budget(
        collection,
        report,
        query,
        db_deadline,
        Duration::from_secs(5),
    )
    .await
}

#[cfg(feature = "test-support")]
pub async fn read_reference_series(
    collection: Collection<Document>,
    report: &Report,
    query: SeriesQuery,
) -> Result<Series> {
    let budget = Duration::from_secs(90);
    tokio::time::timeout(
        budget,
        read_series_with_budget(collection, report, query, budget, budget),
    )
    .await
    .map_err(|_| MetricsError::Busy)?
}

async fn read_series_with_budget(
    collection: Collection<Document>,
    report: &Report,
    query: SeriesQuery,
    db_deadline: Duration,
    fold_budget: Duration,
) -> Result<Series> {
    let top = query.validate(report.scope.admin)?;
    let field = if query.group_by == GroupBy::Owner {
        "owner"
    } else {
        "sandbox_id"
    };
    let selected = match (&query.series_id, query.group_by) {
        (Some(id), _) => vec![id.clone()],
        (None, GroupBy::Total) => Vec::new(),
        (None, _) => {
            ranked_series(&collection, report, query.metric, field, top, db_deadline).await?
        }
    };
    let mut filter = doc! {"report_id":&report.report_id,"metric":query.metric.index() as i64};
    if let Some(id) = &query.series_id {
        filter.insert(field, id);
    }
    let cursor = collection
        .find(filter)
        .sort(doc! {"sandbox_id":1,"chunk":1})
        .batch_size(100)
        .max_time(db_deadline)
        .await
        .map_err(unavailable)?;
    let mut grouped = BTreeMap::<String, Vec<Bucket>>::new();
    let deadline = std::time::Instant::now() + fold_budget;
    fold_sandbox_series(
        cursor,
        report,
        query.metric,
        deadline,
        |sandbox, owner, points| {
            let identity = if query.group_by == GroupBy::Owner {
                owner
            } else {
                sandbox
            };
            let group = if query.group_by == GroupBy::Total {
                "total"
            } else if selected.iter().any(|s| s == identity) {
                identity
            } else {
                "other"
            };
            merge_buckets(grouped.entry(group.into()).or_default(), &points);
            Ok(())
        },
    )
    .await?;
    if grouped.is_empty() && query.series_id.is_some() {
        return Err(MetricsError::NotFound);
    }
    let lines = grouped
        .into_iter()
        .map(|(id, points)| Line {
            label: match id.as_str() {
                "other" => "Other".into(),
                "total" => "Total".into(),
                _ => id.clone(),
            },
            id,
            points: points.into_iter().map(Bucket::point).collect(),
        })
        .collect();
    Ok(Series {
        report_id: report.report_id.clone(),
        metric: query.metric,
        unit: query.metric.unit().into(),
        source: layer2_core::sandbox_metrics::SOURCE.into(),
        lines,
    })
}

pub(super) async fn ranked_series(
    collection: &Collection<Document>,
    report: &Report,
    metric: Metric,
    field: &str,
    top: usize,
    db_deadline: Duration,
) -> Result<Vec<String>> {
    let mut cursor = collection
        .aggregate([
            doc! {"$match":{"report_id":&report.report_id,"metric":metric.index() as i64,"chunk":0_i64}},
            doc! {"$group":{"_id":format!("${field}"),"rank":{"$sum":"$rank"}}},
            doc! {"$sort":{"rank":-1,"_id":1}},
            doc! {"$limit":top as i64},
        ])
        .max_time(db_deadline)
        .await
        .map_err(unavailable)?;
    let mut ids = Vec::new();
    while let Some(doc) = cursor.try_next().await.map_err(unavailable)? {
        ids.push(doc.get_str("_id").map_err(unavailable)?.into());
    }
    Ok(ids)
}

pub(super) async fn fold_sandbox_series(
    mut cursor: mongodb::Cursor<Document>,
    report: &Report,
    metric: Metric,
    deadline: std::time::Instant,
    mut consume: impl FnMut(&str, &str, Vec<Bucket>) -> Result<()>,
) -> Result<()> {
    let mut sandbox = String::new();
    let mut owner = String::new();
    let mut sparse = BTreeMap::new();
    let mut expectations = Vec::new();
    let complete = |sparse: &BTreeMap<u64, Bucket>, expectations: &[Expectation]| {
        series::complete(
            sparse,
            expectations,
            metric,
            report.effective_range,
            report.query.step_seconds,
        )
    };
    while let Some(doc) = cursor.try_next().await.map_err(unavailable)? {
        if std::time::Instant::now() > deadline {
            return Err(MetricsError::Busy);
        }
        let next = doc.get_str("sandbox_id").map_err(unavailable)?;
        if !sandbox.is_empty() && sandbox != next {
            consume(&sandbox, &owner, complete(&sparse, &expectations))?;
            sparse.clear();
            expectations.clear();
        }
        sandbox.clear();
        sandbox.push_str(next);
        owner.clear();
        owner.push_str(doc.get_str("owner").map_err(unavailable)?);
        let record: ChartChunk = decode(&doc)?;
        expectations.extend(record.expectations);
        for point in record.buckets {
            sparse.insert(point.from, point);
        }
    }
    if !sandbox.is_empty() {
        consume(&sandbox, &owner, complete(&sparse, &expectations))?;
    }
    Ok(())
}

pub(super) fn merge_buckets(target: &mut Vec<Bucket>, points: &[Bucket]) {
    if target.is_empty() {
        target.extend_from_slice(points);
    } else {
        for (target, point) in target.iter_mut().zip(points) {
            target.merge(point);
        }
    }
}

#[cfg(test)]
#[path = "chart_expectation_tests.rs"]
mod expectation_tests;
