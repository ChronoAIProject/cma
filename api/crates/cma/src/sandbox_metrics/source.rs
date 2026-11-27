use super::*;
use layer2_core::sandbox_metrics::{
    aggregate, Binding, BindingKind, MetricRollup, Range, Result, ALGORITHM_VERSION, SOURCE,
};
use layer3_adapter::sandbox_metrics::{PhysicalSeries, Selection, SourceMetric};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct SourceRead {
    samples: Vec<PhysicalSeries>,
    ambiguous_sandboxes: BTreeSet<String>,
    complete: bool,
}

#[cfg(test)]
pub(crate) async fn collect_fixture(
    store: Arc<ReportingStore>,
    source: MetricsClient,
    ids: &[String],
    range: Range,
) -> Result<()> {
    let worker = fixture_worker(store.clone(), source);
    let holder = "measured-usage-fixture";
    if !store.lease("rollups", holder, 70).await? {
        return Err(MetricsError::Busy);
    }
    let collected = worker
        .collect_page("rollups", holder, ids, range, false)
        .await;
    store.release("rollups", holder).await?;
    collected
}

#[cfg(test)]
pub(crate) async fn replay_fixture(
    store: Arc<ReportingStore>,
    source: MetricsClient,
    range: Range,
) -> Result<()> {
    let holder = "measured-replay-fixture";
    let key = "replay";
    if !store.lease(key, holder, 70).await? {
        return Err(MetricsError::Busy);
    }
    let collected = async {
        store
            .save_progress(
                key,
                holder,
                &crate::store::usage_reports::Progress {
                    from: range.from,
                    to: range.to,
                    after: None,
                },
            )
            .await?;
        fixture_worker(store.clone(), source)
            .collect(holder, true)
            .await
    }
    .await;
    store.release(key, holder).await?;
    collected
}

#[cfg(test)]
fn fixture_worker(store: Arc<ReportingStore>, source: MetricsClient) -> MetricsWorkers {
    MetricsWorkers {
        measured: None,
        store,
        source: Some(source),
        native: None,
        cluster: "fixture".into(),
        enabled: true,
    }
}

impl MetricsWorkers {
    pub(super) async fn collect(&self, holder: &str, replay: bool) -> Result<()> {
        let now = crate::store::usage_reports::now();
        let cutoff = now.saturating_sub(120) / 60 * 60;
        let key = if replay { "replay" } else { "rollups" };
        let mut progress = self.store.progress(key).await?;
        if progress.to == 0 || progress.from < now.saturating_sub(30 * 86_400) {
            progress = crate::store::usage_reports::Progress::default();
            progress.from = if replay {
                self.store
                    .first_binding_at()
                    .await?
                    .unwrap_or(cutoff)
                    .max(now.saturating_sub(30 * 86_400))
                    / 3600
                    * 3600
            } else {
                (cutoff / 3600 * 3600).saturating_sub(3600)
            };
            progress.to = if replay {
                (progress.from + 3600).min(cutoff)
            } else {
                cutoff
            };
        }
        if replay && progress.after.is_none() && progress.from >= cutoff / 3600 * 3600 {
            progress.to = cutoff;
        }
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(40) {
            let range = Range {
                from: progress.from,
                to: progress.to,
            };
            if range.seconds() == 0 {
                break;
            }
            let ids = self
                .store
                .binding_sandbox_page(progress.after.as_deref(), range)
                .await?;
            if ids.is_empty() {
                if replay && progress.to < cutoff {
                    progress.from = progress.to / 3600 * 3600;
                    progress.to = (progress.from + 3600).min(cutoff);
                    progress.after = None;
                } else if replay {
                    progress.from = cutoff / 3600 * 3600;
                    progress.to = cutoff;
                    progress.after = None;
                } else {
                    progress = Default::default();
                }
                self.store.save_progress(key, holder, &progress).await?;
                break;
            }
            self.collect_page(key, holder, &ids, range, !replay).await?;
            progress.after = ids.last().cloned();
            self.store.save_progress(key, holder, &progress).await?;
        }
        Ok(())
    }
    async fn collect_page(
        &self,
        key: &str,
        holder: &str,
        ids: &[String],
        range: Range,
        include_live: bool,
    ) -> Result<()> {
        let now = crate::store::usage_reports::now();
        let from = range.from;
        let cutoff = range.to;
        let mapping = self.store.mapping_revision().await?;
        let bindings = self.store.bindings(ids, range).await?;
        let source = self
            .read_source(
                &bindings,
                Range {
                    from: from.saturating_sub(120),
                    to: (range.to + 120).min(now),
                },
                false,
            )
            .await?;
        if !source.complete {
            return Err(MetricsError::Unavailable);
        }
        if !source.ambiguous_sandboxes.is_empty() {
            tracing::info!(
                task = key,
                ambiguous_sandboxes = source.ambiguous_sandboxes.len(),
                range_from = from,
                range_to = cutoff,
                "sandbox metric source names were ambiguous"
            );
        }
        let previous_live = if include_live {
            self.store.live_readings(ids).await?
        } else {
            BTreeMap::new()
        };
        let mut rollups = Vec::new();
        let mut readings = Vec::new();
        for id in ids {
            // A name reused across label sets has no unique physical source.
            // Leave its existing observations in place and advance the page.
            if source.ambiguous_sandboxes.contains(id) {
                continue;
            }
            let own: Vec<_> = bindings
                .iter()
                .filter(|b| &b.sandbox_id == id)
                .cloned()
                .collect();
            let Some(owner) = own.first().map(|b| b.owner_subject.clone()) else {
                continue;
            };
            if own.iter().any(|b| b.owner_subject != owner) {
                continue;
            }
            let minutes = aggregate::aggregate(&own, &source.samples, Range { from, to: cutoff })?;
            let mut chunks = BTreeMap::<u64, Vec<_>>::new();
            for minute in minutes {
                chunks
                    .entry(minute.from / 3600 * 3600)
                    .or_default()
                    .push(minute);
            }
            for (hour, minutes) in chunks {
                rollups.push(MetricRollup {
                    schema_version: 1,
                    algorithm_version: ALGORITHM_VERSION,
                    source: SOURCE.into(),
                    sandbox_id: id.clone(),
                    owner_subject: owner.clone(),
                    day: hour / 86_400 * 86_400,
                    chunk: ((hour % 86_400) / 3600) as u16,
                    mapping_revision: mapping,
                    cutoff: cutoff.min(hour + 3600),
                    source_complete: true,
                    minutes,
                });
            }
            if !include_live {
                continue;
            }
            let mut live = aggregate::totals(&aggregate::aggregate(
                &own,
                &source.samples,
                Range {
                    from: now.saturating_sub(120),
                    to: now,
                },
            )?);
            // Preserve each metric independently, including a paused
            // home's last observed bytes when only compute has new data.
            if let Some(old) = previous_live.get(id).filter(|r| r.owner == owner) {
                for (new, old) in live.iter_mut().zip(&old.metrics) {
                    if new.observed_at < old.observed_at {
                        *new = old.clone();
                    }
                }
            }
            if live.iter().any(|v| v.observed_at.is_some()) {
                readings.push((
                    id.clone(),
                    crate::store::sandbox_metric_rollups::LiveReading {
                        owner,
                        observed_at: now,
                        metrics: live,
                    },
                ));
            }
        }
        self.store
            .save_metric_rollups(key, holder, &rollups)
            .await?;
        self.store.save_live(holder, &readings).await?;
        Ok(())
    }
    pub(super) async fn raw(
        &self,
        bindings: &[Binding],
        range: Range,
    ) -> Result<(Vec<PhysicalSeries>, bool)> {
        let read = self.read_source(bindings, range, false).await?;
        Ok((
            read.samples,
            read.complete && read.ambiguous_sandboxes.is_empty(),
        ))
    }

    pub(super) async fn raw_for_measured_evidence(
        &self,
        bindings: &[Binding],
        range: Range,
    ) -> Result<(Vec<PhysicalSeries>, bool)> {
        let read = self.read_source(bindings, range, true).await?;
        Ok((read.samples, read.complete))
    }

    async fn read_source(
        &self,
        bindings: &[Binding],
        range: Range,
        strict: bool,
    ) -> Result<SourceRead> {
        let Some(source) = &self.source else {
            return Ok(SourceRead::default());
        };
        let mut groups = BTreeMap::<_, BTreeMap<String, BTreeSet<&str>>>::new();
        for b in bindings {
            for metric in SourceMetric::ALL {
                if (metric.is_pvc() && b.kind != BindingKind::Home)
                    || (!metric.is_pvc() && b.kind != BindingKind::Compute)
                {
                    continue;
                }
                let name = if metric.is_pvc() {
                    Some(&b.pvc_name)
                } else {
                    b.pod_name.as_ref()
                };
                if let Some(name) = name {
                    groups
                        .entry((b.cluster.clone(), b.namespace.clone(), metric))
                        .or_default()
                        .entry(name.clone())
                        .or_default()
                        .insert(b.sandbox_id.as_str());
                }
            }
        }
        let mut result = SourceRead {
            complete: true,
            ..SourceRead::default()
        };
        for ((cluster, namespace, metric), names) in groups {
            for batch in names.keys().cloned().collect::<Vec<_>>().chunks(100) {
                let selection = Selection {
                    cluster: cluster.clone(),
                    namespace: namespace.clone(),
                    metric,
                    names: batch.to_vec(),
                    from: range.from,
                    to: range.to,
                };
                let holder = format!("vm-read-{}", uuid::Uuid::new_v4().simple());
                let mut slot = None;
                for key in ["vm:0", "vm:1"]
                    .into_iter()
                    .take(self.store.budgets.source_calls as usize)
                {
                    if self.store.lease(key, &holder, 7).await? {
                        slot = Some(key);
                        break;
                    }
                }
                let Some(slot) = slot else {
                    return Err(MetricsError::Busy);
                };
                match tokio::time::timeout(
                    Duration::from_secs(self.store.budgets.source_seconds),
                    source.export_reporting(&selection),
                )
                .await
                {
                    Ok(Ok(export)) => {
                        let refuses_evidence = strict
                            && (!export.ambiguous_names.is_empty()
                                || export.series.iter().any(|series| {
                                    series.samples.iter().any(|sample| sample.value.is_none())
                                }));
                        if refuses_evidence {
                            result.complete = false;
                        } else {
                            for name in export.ambiguous_names {
                                if let Some(ids) = names.get(&name) {
                                    result
                                        .ambiguous_sandboxes
                                        .extend(ids.iter().map(|id| (*id).to_owned()));
                                }
                            }
                            result.samples.extend(export.series);
                        }
                    }
                    _ => result.complete = false,
                }
                self.store.release(slot, &holder).await?;
            }
        }
        Ok(result)
    }
}
