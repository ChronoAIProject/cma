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
pub(crate) fn chart_documents(
    report: &Report,
    sandbox: &str,
    owner: &str,
    metric: Metric,
    buckets: Vec<Bucket>,
    expectations: &[Expectation],
) -> Result<Vec<Document>> {
    let rank: f64 = buckets.iter().map(|b| b.integral).sum();
    let mut out = Vec::new();
    // A membership marker is required even if all measurements are absent.
    // Its compact intervals reconstruct null buckets without caching zeros.
    for chunk in 0..buckets.len().div_ceil(168).max(1) {
        let points =
            &buckets[(chunk * 168).min(buckets.len())..((chunk + 1) * 168).min(buckets.len())];
        let record = ChartChunk {
            buckets: points.to_vec(),
            expectations: if chunk == 0 {
                expectations.to_vec()
            } else {
                Vec::new()
            },
        };
        out.push(doc!{"_id":format!("{}:{sandbox}:{}:{chunk}",report.report_id,metric.index()),"document_schema_version":1_i64,
            "report_id":&report.report_id,"sandbox_id":sandbox,"owner":owner,"group":"sandbox","series_id":sandbox,
            "metric":metric.index() as i64,"chunk":chunk as i64,"rank":rank,"expires_at":expires(report.expires_at+60),"record":encode(&record)?});
    }
    Ok(out)
}
impl ReportingStore {
    pub(super) async fn read_series(&self, report: &Report, query: SeriesQuery) -> Result<Series> {
        let top = query.validate(report.scope.admin)?;
        let field = if query.group_by == GroupBy::Owner {
            "owner"
        } else {
            "sandbox_id"
        };
        let selected: Vec<String> = if let Some(id) = &query.series_id {
            vec![id.clone()]
        } else if query.group_by == GroupBy::Total {
            Vec::new()
        } else {
            let mut c=self.collection(CHARTS).aggregate([doc!{"$match":{"report_id":&report.report_id,"metric":query.metric.index() as i64,"chunk":0_i64}},
                doc!{"$group":{"_id":format!("${field}"),"rank":{"$sum":"$rank"}}},doc!{"$sort":{"rank":-1,"_id":1}},doc!{"$limit":top as i64}]).max_time(DB_DEADLINE).await.map_err(unavailable)?;
            let mut ids = Vec::new();
            while let Some(d) = c.try_next().await.map_err(unavailable)? {
                ids.push(d.get_str("_id").map_err(unavailable)?.into());
            }
            ids
        };
        let mut filter = doc! {"report_id":&report.report_id,"metric":query.metric.index() as i64};
        if let Some(id) = &query.series_id {
            filter.insert(field, id);
        }
        let mut cursor = self
            .collection(CHARTS)
            .find(filter)
            .sort(doc! {"sandbox_id":1,"chunk":1})
            .batch_size(100)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        let mut grouped = BTreeMap::<String, Vec<Bucket>>::new();
        let mut sandbox = String::new();
        let mut group = String::new();
        let mut sparse = BTreeMap::new();
        let mut expectations = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Some(doc) = cursor.try_next().await.map_err(unavailable)? {
            if std::time::Instant::now() > deadline {
                return Err(MetricsError::Busy);
            }
            let next = doc.get_str("sandbox_id").map_err(unavailable)?;
            if !sandbox.is_empty() && sandbox != next {
                merge_sandbox(
                    &mut grouped,
                    &group,
                    &sparse,
                    &expectations,
                    report,
                    query.metric,
                );
                sparse.clear();
                expectations.clear();
            }
            sandbox = next.into();
            let identity = doc.get_str(field).map_err(unavailable)?;
            group = if query.group_by == GroupBy::Total {
                "total"
            } else if selected.iter().any(|s| s == identity) {
                identity
            } else {
                "other"
            }
            .into();
            let record: ChartChunk = decode(&doc)?;
            expectations.extend(record.expectations);
            for p in record.buckets {
                sparse.insert(p.from, p);
            }
        }
        if !sandbox.is_empty() {
            merge_sandbox(
                &mut grouped,
                &group,
                &sparse,
                &expectations,
                report,
                query.metric,
            );
        } else if query.series_id.is_some() {
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
}
fn merge_sandbox(
    grouped: &mut BTreeMap<String, Vec<Bucket>>,
    group: &str,
    sparse: &BTreeMap<u64, Bucket>,
    expectations: &[Expectation],
    report: &Report,
    metric: Metric,
) {
    let complete = series::complete(
        sparse,
        expectations,
        metric,
        report.effective_range,
        report.query.step_seconds,
    );
    let target = grouped.entry(group.into()).or_default();
    if target.is_empty() {
        *target = complete;
    } else {
        for (a, b) in target.iter_mut().zip(&complete) {
            a.merge(b);
        }
    }
}
