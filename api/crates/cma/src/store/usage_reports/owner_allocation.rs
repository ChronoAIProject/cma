//! Bounded owner rankings over the frozen report, never a new collection job.
use super::{aggregate_projections, *};
use cma_am_reporting::ProjectionBatch;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    cursor::Cursor, disambiguate_options_with_context, option_label, FilterOption, Metric,
    OptionContext, OptionKind, Order, OwnerAllocation, OwnerAllocationQuery, OwnerAllocations,
    OwnerContextAccess, Report, Sort,
};
use std::time::Instant;

impl ReportingStore {
    pub(super) async fn read_owner_allocations(
        &self,
        report: &Report,
        query: OwnerAllocationQuery,
        owner_context: OwnerContextAccess,
    ) -> Result<OwnerAllocations> {
        if !report.scope.admin {
            return Err(MetricsError::Forbidden);
        }
        let limit = query.validate()?;
        let _timer = self.time_read("owner_allocations")?;
        let hash = format!("owner-allocations-v1:{}", query.field());
        let sort = match query.metric {
            Metric::Cpu => Sort::AllocatedCpuCoreHours,
            Metric::Memory => Sort::AllocatedMemoryGibHours,
            Metric::Disk => Sort::AllocatedDiskGibHours,
        };
        let after = query
            .cursor
            .as_deref()
            .map(|token| {
                self.signer.decode(
                    token,
                    &report.scope,
                    &report.report_id,
                    &hash,
                    sort,
                    Order::Desc,
                )
            })
            .transpose()?;
        let mut page = Vec::new();
        if let Some(after) = after {
            let value = after
                .last_value
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| MetricsError::Invalid("Invalid owner allocation cursor.".into()))?;
            page.push(doc! {"$match":{"$or":[
                {"allocated_hours":{"$lt":value}},
                {"allocated_hours":value,"_id":{"$gt":after.sandbox_id}}
            ]}});
        }
        page.extend([
            doc! {"$sort":{"allocated_hours":-1,"_id":1}},
            doc! {"$limit":i64::from(limit)+1},
        ]);
        let projected = self.projected_generation(report).await?;
        let (collection, mut pipeline) = if let Some(generation) = projected {
            (
                aggregate_projections::COLLECTION,
                vec![
                    doc! {"$match":{"report_id":&report.report_id,"generation":generation,
                    "document_schema_version":1_i64,"kind":"owner_allocation","metric":query.metric.index() as i64}},
                    doc! {"$project":{"_id":"$series","allocated_hours":1,"sandbox_count":1,"complete":1}},
                ],
            )
        } else {
            (ROWS, owner_totals(report, query.field()).into())
        };
        // Resolve names across the entire cohort, including equal-context
        // ties whose owners appear on different pages.
        let mut facets = doc! {"page":page,"count":[{"$count":"total"}],"identities":[
            {"$project":{"_id":1}},
            {"$sort":{"_id":1}},
            {"$lookup":{"from":"users","localField":"_id","foreignField":"_id","as":"identity"}},
            {"$project":{"name":{"$arrayElemAt":["$identity.display_name",0]},
                "first_seen_at":{"$arrayElemAt":["$identity.first_seen_at",0]}}}
        ]};
        if projected.is_some() {
            facets.insert(
                "coverage",
                vec![doc! {"$group":{"_id":null,"members":{"$sum":"$sandbox_count"}}}],
            );
        }
        pipeline.push(doc! {"$facet":facets});
        // Mongo errors can include user data. Do not log driver errors here.

        #[cfg(test)]
        let query_started = std::time::Instant::now();
        let mut cursor = self
            .collection(collection)
            .aggregate(pipeline)
            .allow_disk_use(false)
            .max_time(DB_DEADLINE)
            .await
            .map_err(|_| MetricsError::Unavailable)?;
        let document = cursor
            .try_next()
            .await
            .map_err(|_| MetricsError::Unavailable)?
            .ok_or(MetricsError::Unavailable)?;
        if projected.is_some() {
            let members = document
                .get_array("coverage")
                .map_err(unavailable)?
                .first()
                .and_then(|value| value.as_document())
                .and_then(|value| value.get_i64("members").ok())
                .unwrap_or(0);
            let expected = report
                .summary
                .as_ref()
                .ok_or(MetricsError::Unavailable)?
                .sandbox_count;
            if u64::try_from(members).ok() != Some(expected) {
                return Err(MetricsError::Unavailable);
            }
        }
        #[cfg(test)]
        tracing::info!(target: "cma::store::usage_reports::budget", reason = "owner_aggregate_sample", probe = true, elapsed_ms = query_started.elapsed().as_millis() as u64);
        #[cfg(test)]
        let labels_started = std::time::Instant::now();
        let total_count = document
            .get_array("count")
            .map_err(|_| MetricsError::Unavailable)?
            .first()
            .and_then(|v| v.as_document())
            .and_then(|v| {
                v.get_i64("total")
                    .or_else(|_| v.get_i32("total").map(i64::from))
                    .ok()
            })
            .unwrap_or(0) as u64;
        let mut labels = Vec::new();
        let mut contexts = Vec::new();
        for value in document
            .get_array("identities")
            .map_err(|_| MetricsError::Unavailable)?
        {
            let row = value.as_document().ok_or(MetricsError::Unavailable)?;
            let subject = row.get_str("_id").map_err(|_| MetricsError::Unavailable)?;
            labels.push(FilterOption {
                value: subject.into(),
                label: option_label(OptionKind::Owner, subject, row.get_str("name").ok()),
                disambiguator: None,
            });
            contexts.push(OptionContext::owner_with_access(
                row.get_str("first_seen_at").ok(),
                owner_context,
            ));
        }
        disambiguate_options_with_context(&mut labels, &contexts);
        let mut labels: std::collections::HashMap<_, _> = labels
            .into_iter()
            .map(|label| (label.value.clone(), label))
            .collect();
        #[cfg(test)]
        tracing::info!(target: "cma::store::usage_reports::budget", reason = "owner_labels_sample", probe = true, elapsed_ms = labels_started.elapsed().as_millis() as u64);
        let mut items = Vec::new();
        for value in document
            .get_array("page")
            .map_err(|_| MetricsError::Unavailable)?
        {
            let row = value.as_document().ok_or(MetricsError::Unavailable)?;
            let subject = row.get_str("_id").map_err(|_| MetricsError::Unavailable)?;
            let allocated_hours = match row.get("allocated_hours") {
                Some(mongodb::bson::Bson::Double(v)) => *v,
                Some(mongodb::bson::Bson::Int64(v)) => *v as f64,
                Some(mongodb::bson::Bson::Int32(v)) => f64::from(*v),
                _ => return Err(MetricsError::Unavailable),
            };
            if !allocated_hours.is_finite() || allocated_hours < 0.0 {
                return Err(MetricsError::Unavailable);
            }
            let label = labels.remove(subject).ok_or(MetricsError::Unavailable)?;
            items.push(OwnerAllocation {
                subject: subject.into(),
                label: label.label,
                disambiguator: label.disambiguator,
                allocated_hours,
                sandbox_count: row
                    .get_i64("sandbox_count")
                    .map_err(|_| MetricsError::Unavailable)? as u64,
                complete: row
                    .get_bool("complete")
                    .map_err(|_| MetricsError::Unavailable)?,
            });
        }
        let next = if items.len() > usize::from(limit) {
            items.pop();
            let last = items.last().ok_or(MetricsError::Unavailable)?;
            Some(self.signer.encode(&Cursor {
                principal: report.scope.principal.clone(),
                admin: true,
                filter_hash: hash,
                report_id: report.report_id.clone(),
                sort,
                order: Order::Desc,
                last_value: last.allocated_hours.into(),
                last_null: false,
                sandbox_id: last.subject.clone(),
            })?)
        } else {
            None
        };
        Ok(OwnerAllocations {
            report_id: report.report_id.clone(),
            metric: query.metric,
            unit: query.unit().into(),
            items,
            next,
            total_count,
        })
    }

    pub(super) async fn build_owner_allocation_projections(
        &self,
        report: &Report,
        base: &Document,
        batch: &mut ProjectionBatch,
        deadline: Instant,
    ) -> Result<()> {
        for (metric, field) in [
            (Metric::Cpu, "cpu_core_hours"),
            (Metric::Memory, "memory_gib_hours"),
            (Metric::Disk, "disk_gib_hours"),
        ] {
            let mut cursor = self
                .collection(ROWS)
                .aggregate(owner_totals(report, field))
                .allow_disk_use(false)
                .max_time(DB_DEADLINE)
                .await
                .map_err(|_| MetricsError::Unavailable)?;
            while let Some(mut row) = cursor
                .try_next()
                .await
                .map_err(|_| MetricsError::Unavailable)?
            {
                if Instant::now() >= deadline {
                    return Err(MetricsError::Busy);
                }
                let owner = row
                    .remove("_id")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .ok_or(MetricsError::Unavailable)?;
                let mut document = base.clone();
                document.extend(
                    doc! {"kind":"owner_allocation","metric":metric.index() as i64,
                    "series":owner,"chunk":0_i64},
                );
                document.extend(row);
                batch.push(document)?;
            }
        }
        Ok(())
    }
}

fn owner_totals(report: &Report, field: &str) -> [Document; 2] {
    [
        doc! {"$match":{"report_id":&report.report_id}},
        doc! {"$group":{"_id":"$owner",
        "allocated_hours":{"$sum":format!("$record.allocated_period.{field}")},
        "sandbox_count":{"$sum":1_i64},
        "complete":{"$min":"$record.allocated_period.complete"}}},
    ]
}
