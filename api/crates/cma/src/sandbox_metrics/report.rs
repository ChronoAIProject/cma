use super::projection::{add_summary, Accumulator};
use super::report_trace::{report_hash, ReportTrace, Stage};
use super::*;
use crate::store::usage_reports::{chart_documents, now, ReportLease};
use layer2_core::sandbox_metrics::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
impl MetricsWorkers {
    pub(super) async fn reports(&self, slot: u8) {
        let holder = format!("report-{}-{slot}", uuid::Uuid::new_v4().simple());
        let slot = format!("report:{slot}");
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match self.store.claim_report(&slot, &holder).await {
                Ok(Some(lease)) => {
                    let start = std::time::Instant::now();
                    let generation = tokio::time::timeout(
                        Duration::from_secs(self.store.budgets.report_seconds),
                        self.generate(&lease),
                    )
                    .await;
                    let failure_class = match &generation {
                        Err(_) => "deadline",
                        Ok(Ok(_)) => "none",
                        Ok(Err(MetricsError::Unavailable)) => "unavailable",
                        Ok(Err(MetricsError::Busy)) => "budget",
                        Ok(Err(MetricsError::Expired)) => "expired",
                        Ok(Err(MetricsError::Forbidden | MetricsError::NotFound)) => "authority",
                        Ok(Err(MetricsError::Invalid(_))) => "invalid",
                    };
                    let result = generation.unwrap_or(Err(MetricsError::Unavailable));
                    let generation_ms = start.elapsed().as_millis() as u64;
                    let digest = report_hash(&lease.report.report_id);
                    if let Err(error) = &result {
                        tracing::warn!(error=%error, "sandbox report generation failed");
                    }
                    let generation_success = result.is_ok();
                    let report = result.unwrap_or_else(|_| lease.report.clone());
                    let finish_started = std::time::Instant::now();
                    let finalized = self
                        .store
                        .finish_report(&lease, report, generation_success)
                        .await;
                    if let Err(error) = &finalized {
                        tracing::warn!(error=%error,"sandbox report finalization failed");
                    }
                    let _root = tracing::info_span!(parent: None, "usage_report_diagnostic").entered();
                    tracing::info!(
                        parent: None,
                        report_hash = %digest,
                        success = generation_success && finalized.is_ok(),
                        generation_success,
                        finalized = finalized.is_ok(),
                        failure_class,
                        generation_ms,
                        finalization_ms = finish_started.elapsed().as_millis() as u64,
                        elapsed_ms = start.elapsed().as_millis() as u64,
                        "sandbox usage report generated"
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(error=%error,"sandbox report queue unavailable");
                }
            }
        }
    }
    pub(super) async fn generate(&self, lease: &ReportLease) -> Result<Report> {
        let mut trace = ReportTrace::new(&lease.report.report_id);
        let result = self.generate_traced(lease, &mut trace).await;
        trace.finish(result.is_ok());
        result
    }
    async fn generate_traced(
        &self,
        lease: &ReportLease,
        trace: &mut ReportTrace,
    ) -> Result<Report> {
        let mut report = lease.report.clone();
        let mut snapshot = trace.stage(Stage::Inventory, self.store.snapshot()).await?;
        if self.enabled {
            report.mapping_revision = trace
                .stage(
                    Stage::Measured,
                    self.store.mapping_revision_at(&mut snapshot),
                )
                .await?;
        }
        let mut summary = Summary {
            allocated_period: AllocatedPeriod {
                complete: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut aggregate = super::summary::Fleet::default();
        let mut watermark = Sha256::new();
        let mut after = None;
        let mut source_complete = self.enabled;
        loop {
            let page = trace
                .stage(
                    Stage::Inventory,
                    self.store.inventory(
                        &mut snapshot,
                        &report.scope,
                        &report.query,
                        after.as_deref(),
                    ),
                )
                .await?;
            if page.is_empty() {
                break;
            }
            after = page.last().map(|i| i.state.sandbox_id.to_string());
            let selected: Vec<_> = page
                .into_iter()
                .filter(|i| i.matches(&report.query))
                .collect();
            let ids: Vec<_> = selected
                .iter()
                .map(|i| i.state.sandbox_id.to_string())
                .collect();
            let states: Vec<_> = selected.iter().map(|i| i.state.clone()).collect();
            if ids.is_empty() {
                continue;
            }
            if self.enabled {
                if let Some(first) = trace
                    .stage(
                        Stage::Measured,
                        self.store.tracking_started_at(&ids, &mut snapshot),
                    )
                    .await?
                {
                    report.tracking_started_at = Some(
                        report
                            .tracking_started_at
                            .map_or(first, |old| old.min(first)),
                    );
                }
            }
            let live = if self.enabled {
                trace
                    .stage(
                        Stage::Measured,
                        self.store.live_readings_at(&ids, &mut snapshot),
                    )
                    .await?
            } else {
                BTreeMap::new()
            };
            let mut accum: BTreeMap<_, Accumulator> = ids
                .iter()
                .cloned()
                .map(|id| (id, Accumulator::default()))
                .collect();
            let mut from = report.effective_range.from;
            while from < report.effective_range.to {
                let range = Range {
                    from,
                    to: ((from / 86_400 + 1) * 86_400).min(report.effective_range.to),
                };
                let allocation = trace
                    .stage(
                        Stage::Allocation,
                        self.store.allocation_periods(&mut snapshot, &states, range),
                    )
                    .await?;
                for (id, value) in allocation {
                    if let Some(a) = accum.get_mut(&id) {
                        a.allocation.merge(&value.period);
                        a.expectations.extend(value.expectations);
                    }
                }
                if self.enabled {
                    let rollups = trace
                        .stage(
                            Stage::Measured,
                            self.store.metric_rollups(&mut snapshot, &ids, range),
                        )
                        .await?;
                    let mut minutes = BTreeMap::<String, Vec<Minute>>::new();
                    for rollup in rollups {
                        if rollup.mapping_revision > report.mapping_revision {
                            return Err(MetricsError::Unavailable);
                        }
                        let Some(state) = states.iter().find(|s| {
                            s.sandbox_id.as_str() == rollup.sandbox_id
                                && s.owner.as_str() == rollup.owner_subject
                        }) else {
                            continue;
                        };
                        source_complete &= rollup.source_complete;
                        for minute in rollup.minutes {
                            if minute.from >= range.from
                                && minute.to <= range.to
                                && minute.to <= rollup.cutoff
                            {
                                minutes
                                    .entry(state.sandbox_id.to_string())
                                    .or_default()
                                    .push(minute);
                            }
                        }
                    }
                    // Raw reads repair only the at-most-two partial minute edges.
                    // Interior history always uses reusable minute rollups.
                    let mut edges = Vec::new();
                    if !range.from.is_multiple_of(60) {
                        edges.push(Range {
                            from: range.from,
                            to: (range.from / 60 * 60 + 60).min(range.to),
                        });
                    }
                    if !range.to.is_multiple_of(60) {
                        let edge = Range {
                            from: (range.to / 60 * 60).max(range.from),
                            to: range.to,
                        };
                        if !edges.contains(&edge) {
                            edges.push(edge);
                        }
                    }
                    for edge in edges {
                        let bindings = trace
                            .stage(
                                Stage::Measured,
                                self.store.bindings_at(&ids, edge, &mut snapshot),
                            )
                            .await?;
                        let (samples, complete) = trace
                            .stage(
                                Stage::Source,
                                self.raw(
                                    &bindings,
                                    Range {
                                        from: edge.from.saturating_sub(120),
                                        to: (edge.to + 90).min(report.as_of),
                                    },
                                ),
                            )
                            .await?;
                        source_complete &= complete;
                        for id in &ids {
                            let own: Vec<_> = bindings
                                .iter()
                                .filter(|b| &b.sandbox_id == id)
                                .cloned()
                                .collect();
                            minutes
                                .entry(id.clone())
                                .or_default()
                                .extend(aggregate::aggregate(&own, &samples, edge)?);
                        }
                    }
                    for (id, minutes) in minutes {
                        if let Some(a) = accum.get_mut(&id) {
                            a.add(&minutes, &report.query, range);
                        }
                    }
                }
                from = range.to;
            }
            let mut pending_rows = Vec::new();
            let mut pending_charts = Vec::new();
            for item in &selected {
                watermark.update(item.state.sandbox_id.as_str());
                watermark.update(item.state.revision.to_be_bytes());
                let a = accum
                    .remove(item.state.sandbox_id.as_str())
                    .ok_or(MetricsError::Unavailable)?;
                let row = a.row(item, live.get(item.state.sandbox_id.as_str()), &report);
                let mut charts = Vec::new();
                for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
                    charts.extend(chart_documents(
                        &report,
                        &row.sandbox_id,
                        &row.owner.subject,
                        metric,
                        a.buckets[metric.index()].values().cloned().collect(),
                        &a.expectations,
                    )?);
                }
                add_summary(&mut summary, &row, &mut aggregate, &a);
                pending_rows.push(row);
                pending_charts.extend(charts);
                if pending_rows.len() == 100 {
                    trace
                        .stage(
                            Stage::Write,
                            self.store.write_report_chunk(
                                lease,
                                &pending_rows,
                                std::mem::take(&mut pending_charts),
                            ),
                        )
                        .await?;
                    pending_rows.clear();
                }
            }
            if !pending_rows.is_empty() {
                trace
                    .stage(
                        Stage::Write,
                        self.store
                            .write_report_chunk(lease, &pending_rows, pending_charts),
                    )
                    .await?;
            }
        }
        summary.measured = Some(aggregate.measured(report.as_of, summary.running_count > 0));
        if self.enabled && report.scope.admin {
            summary.infrastructure = Some(
                trace
                    .stage(
                        Stage::Measured,
                        self.store.infrastructure_at(
                            &mut snapshot,
                            report.mapping_revision,
                            report.as_of,
                        ),
                    )
                    .await?,
            );
        }
        report.allocation_watermark = format!("{:x}", watermark.finalize());
        report.available_from = aggregate
            .totals
            .iter()
            .filter_map(|a| a.available_from.map(|v| v as u64))
            .min();

        report.source_health =
            if source_complete && (summary.sandbox_count == 0 || report.available_from.is_some()) {
                "ok"
            } else {
                "source_unavailable"
            }
            .into();
        if report.source_health != "ok" {
            report.warnings.push("source_unavailable".into());
        }
        if summary.excluded_count > 0 {
            report
                .warnings
                .push("measurement_coverage_incomplete".into());
        }
        if !summary.allocated_period.complete {
            report.warnings.push("allocation_history_incomplete".into());
        }
        if report.effective_range.seconds() == 0 {
            report.warnings.push("no_finalized_samples".into());
        }
        tracing::info!(sandboxes=summary.sandbox_count,excluded=summary.excluded_count,
            source_health=%report.source_health,mapping_revision=report.mapping_revision,
            cpu_covered=aggregate.totals[0].covered_seconds,cpu_expected=aggregate.totals[0].expected_seconds,
            memory_covered=aggregate.totals[1].covered_seconds,disk_covered=aggregate.totals[2].covered_seconds,
            "sandbox report coverage");
        report.summary = Some(summary);
        trace
            .stage(Stage::Authority, self.store.reauthorize(&report))
            .await?;
        if now() >= report.expires_at {
            return Err(MetricsError::Expired);
        }
        Ok(report)
    }
}
