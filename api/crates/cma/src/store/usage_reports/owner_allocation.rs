//! Bounded owner rankings over the frozen report, never a new collection job.
use super::{aggregate_projections, *};
use cma_am_reporting::{ProjectionBatch, ProjectionManifest};
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
        if let Some(after) = after.as_ref() {
            let value = after
                .last_value
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| MetricsError::Invalid("Invalid owner allocation cursor.".into()))?;
            page.push(doc! {"$match":{"$or":[
                {"allocated_hours":{"$lt":value}},
                {"allocated_hours":value,"_id":{"$gt":&after.sandbox_id}}
            ]}});
        }
        page.extend([
            doc! {"$sort":{"allocated_hours":-1,"_id":1}},
            doc! {"$limit":i64::from(limit)+1},
        ]);
        let projected = self.projected_family(report, "owner_allocation").await?;
        let (collection, mut pipeline) = if let Some(manifest) = projected.as_ref() {
            (
                aggregate_projections::COLLECTION,
                vec![
                    doc! {"$match":{"report_id":&report.report_id,"generation":manifest.generation,
                    "document_schema_version":1_i64,"kind":"owner_allocation","metric":query.metric.index() as i64}},
                    doc! {"$project":{"_id":"$series","allocated_hours":1,"sandbox_count":1,"complete":1,"projection_document":"$$ROOT"}},
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
        if let Some(manifest) = projected.as_ref() {
            verify_allocation_page(manifest, query.metric, after.as_ref(), limit, &document)?;
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
            let mut pipeline = owner_totals(report, field).to_vec();
            pipeline.push(doc! {"$sort":{"allocated_hours":-1,"_id":1}});
            let mut cursor = self
                .collection(ROWS)
                .aggregate(pipeline)
                .allow_disk_use(false)
                .max_time(DB_DEADLINE)
                .await
                .map_err(cma_am_reporting::optional_query_error)?;
            let mut rank = 0_i64;
            while let Some(mut row) = cursor
                .try_next()
                .await
                .map_err(cma_am_reporting::optional_query_error)?
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
                    "series":owner,"chunk":0_i64,"allocation_rank":rank},
                );
                document.extend(row);
                batch.push(document)?;
                rank += 1;
            }
        }
        Ok(())
    }
}

fn verify_allocation_page(
    manifest: &ProjectionManifest,
    metric: Metric,
    after: Option<&Cursor>,
    limit: u16,
    response: &Document,
) -> Result<()> {
    let metric = metric.index() as i64;
    let order = manifest.allocation_order(metric)?;
    let entries: Vec<&Document> = order
        .iter()
        .map(|entry| entry.as_document().ok_or(MetricsError::Unavailable))
        .collect::<Result<_>>()?;
    let mut membership = std::collections::BTreeSet::new();
    for entry in &entries {
        if !membership.insert(entry.get_str("owner").map_err(unavailable)?) {
            return Err(MetricsError::Unavailable);
        }
    }
    let actual = response.get_array("identities").map_err(unavailable)?;
    if actual.len() != membership.len() {
        return Err(MetricsError::Unavailable);
    }
    for (value, expected) in actual.iter().zip(membership) {
        if value.as_document().and_then(|row| row.get_str("_id").ok()) != Some(expected) {
            return Err(MetricsError::Unavailable);
        }
    }
    let count = response
        .get_array("count")
        .map_err(unavailable)?
        .first()
        .and_then(|value| value.as_document())
        .and_then(|row| {
            row.get_i64("total")
                .or_else(|_| row.get_i32("total").map(i64::from))
                .ok()
        })
        .unwrap_or(0);
    if count != entries.len() as i64 {
        return Err(MetricsError::Unavailable);
    }
    let start = if let Some(after) = after {
        let position = entries
            .iter()
            .position(|entry| entry.get_str("owner").ok() == Some(after.sandbox_id.as_str()))
            .ok_or(MetricsError::Unavailable)?;
        let amount = allocation_number(
            entries[position]
                .get("amount")
                .ok_or(MetricsError::Unavailable)?,
        )?;
        if after.last_value.as_f64().map(f64::to_bits) != Some(amount.to_bits()) {
            return Err(MetricsError::Unavailable);
        }
        position + 1
    } else {
        0
    };
    let end = entries.len().min(start + usize::from(limit) + 1);
    let expected = &entries[start..end];
    let page = response.get_array("page").map_err(unavailable)?;
    if page.len() != expected.len() {
        return Err(MetricsError::Unavailable);
    }
    for (index, (row, expected)) in page.iter().zip(expected).enumerate() {
        let row = row.as_document().ok_or(MetricsError::Unavailable)?;
        let owner = expected.get_str("owner").map_err(unavailable)?;
        let source = row
            .get_document("projection_document")
            .map_err(unavailable)?;
        if row.get_str("_id").ok() != Some(owner)
            || source.get_str("series").ok() != Some(owner)
            || source.get_i64("allocation_rank").ok() != Some((start + index) as i64)
            || source.get("allocated_hours") != expected.get("amount")
            || ["allocated_hours", "sandbox_count", "complete"]
                .iter()
                .any(|key| row.get(*key) != source.get(*key))
        {
            return Err(MetricsError::Unavailable);
        }
        manifest.verify_document("owner_allocation", metric, owner, source)?;
    }
    Ok(())
}

fn allocation_number(value: &mongodb::bson::Bson) -> Result<f64> {
    match value {
        mongodb::bson::Bson::Double(value) => Ok(*value),
        mongodb::bson::Bson::Int64(value) => Ok(*value as f64),
        mongodb::bson::Bson::Int32(value) => Ok(f64::from(*value)),
        _ => Err(MetricsError::Unavailable),
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

#[cfg(test)]
mod tests {
    use super::*;
    use mongodb::bson::RawDocumentBuf;

    #[test]
    fn allocation_page_requires_the_committed_order_and_lookahead() {
        let owners = [("a", 3.0), ("b", 2.0), ("c", 1.0)];
        let documents: Vec<_> = owners.iter().enumerate().map(|(rank, (owner, amount))| doc! {
            "generation":1_i64,"kind":"owner_allocation","metric":0_i64,"series":owner,
            "chunk":0_i64,"allocation_rank":rank as i64,"allocated_hours":amount,"sandbox_count":1_i64,"complete":true,
        }).collect();
        let raw: Vec<_> = documents
            .iter()
            .map(|doc| RawDocumentBuf::try_from(doc).unwrap())
            .collect();
        let bytes = raw.iter().map(|doc| doc.as_bytes().len() as i64).sum();
        let manifest = ProjectionManifest::build(1, &raw, bytes).unwrap();
        let manifest = ProjectionManifest::parse(1, &manifest).unwrap();
        let row = |index: usize| {
            doc! {"_id":owners[index].0,"allocated_hours":owners[index].1,
            "sandbox_count":1_i64,"complete":true,"projection_document":&documents[index]}
        };
        let response = |page: Vec<Document>| doc! {"page":page,"count":[{"total":3}],"identities":[{"_id":"a"},{"_id":"b"},{"_id":"c"}]};
        verify_allocation_page(
            &manifest,
            Metric::Cpu,
            None,
            1,
            &response(vec![row(0), row(1)]),
        )
        .unwrap();
        assert!(matches!(
            verify_allocation_page(&manifest, Metric::Cpu, None, 1, &response(vec![row(0)])),
            Err(MetricsError::Unavailable)
        ));
        let after = Cursor {
            principal: "fixture".into(),
            admin: true,
            filter_hash: String::new(),
            report_id: "fixture".into(),
            sort: Sort::AllocatedCpuCoreHours,
            order: Order::Desc,
            last_value: 3.0.into(),
            last_null: false,
            sandbox_id: "a".into(),
        };
        verify_allocation_page(
            &manifest,
            Metric::Cpu,
            Some(&after),
            1,
            &response(vec![row(1), row(2)]),
        )
        .unwrap();
        assert!(matches!(
            verify_allocation_page(
                &manifest,
                Metric::Cpu,
                Some(&after),
                1,
                &response(vec![row(2)])
            ),
            Err(MetricsError::Unavailable)
        ));
    }
}
