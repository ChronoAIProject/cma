//! Admin-only current infrastructure sums. No customer identity is inferred.
use super::*;
use layer2_core::sandbox_metrics::*;
use layer3_adapter::agent_sandbox::MetricCandidate;
use std::collections::BTreeSet;
impl MetricsWorkers {
    pub(super) async fn diagnostics(&self) -> Result<()> {
        let Some(native) = &self.native else {
            return Ok(());
        };
        let revision = self.store.mapping_revision().await?;
        let mut totals = Infrastructure {
            state: "ok".into(),
            ..Default::default()
        };
        let mut unassigned_count = 0_u64;
        let mut unattributed_count = 0_u64;
        let mut after = None;
        let mut seen = BTreeSet::new();
        let mut physical = BTreeSet::new();
        loop {
            let page = native
                .metric_candidates(after.as_deref())
                .await
                .map_err(|_| MetricsError::Unavailable)?;
            totals.unproven_count += page.unproven;
            let at = crate::store::usage_reports::now();
            let mut bindings = Vec::new();
            for candidate in page.items {
                for kind in [BindingKind::Compute, BindingKind::Home] {
                    let b = binding(&candidate, kind, &self.cluster, at);
                    // Multiple mounts and an overlapping Pod rollout cannot
                    // multiply a single physical PVC in this footer.
                    if physical.insert(b.physical_key()) {
                        bindings.push(b);
                    }
                }
            }
            let bindings = self.store.diagnostic_bindings(bindings).await?;
            let range = Range {
                from: at.saturating_sub(120),
                to: at,
            };
            let (samples, complete) = self.raw(&bindings, range).await?;
            if !complete {
                totals.state = "source_unavailable".into();
            }
            for b in bindings {
                if b.kind == BindingKind::Compute {
                    if b.owner_subject == "unassigned_pool" {
                        unassigned_count += 1;
                    } else {
                        unattributed_count += 1;
                    }
                }
                let values = aggregate::totals(&aggregate::aggregate(
                    std::slice::from_ref(&b),
                    &samples,
                    range,
                )?);
                let destination = if b.owner_subject == "unassigned_pool" {
                    &mut totals.unassigned_pool
                } else {
                    &mut totals.unattributed
                };
                for (i, value) in values.into_iter().enumerate() {
                    if (b.kind == BindingKind::Home) != (i == 2) {
                        continue;
                    }
                    let measured = Measurement::project(
                        &value,
                        [Metric::Cpu, Metric::Memory, Metric::Disk][i],
                        at,
                        true,
                        0.,
                    );
                    if let Some(v) = measured.latest {
                        destination[i] = Some(destination[i].unwrap_or_default() + v);
                        if let Some(sample) = measured.observed_at {
                            totals.observed_at = Some(
                                totals
                                    .observed_at
                                    .map_or(sample as u64, |old| old.min(sample as u64)),
                            );
                        }
                    } else {
                        totals.unproven_count += 1;
                    }
                }
            }
            after = page.next;
            let Some(next) = after.as_ref() else {
                break;
            };
            if !seen.insert(next.clone()) {
                return Err(MetricsError::Unavailable);
            }
        }
        if totals.unproven_count > 0 && totals.state == "ok" {
            totals.state = "partial".into();
        }
        // A changing assignment invalidates the entire diagnostic footer;
        // the following cycle retries it against one mapping revision.
        if self.store.mapping_revision().await? != revision {
            return Err(MetricsError::Unavailable);
        }
        tracing::info!(unassigned_count,unattributed_count,unproven_count=totals.unproven_count,state=%totals.state,"sandbox infrastructure diagnostics");
        self.store.save_infrastructure(&totals, revision).await
    }
}
fn binding(c: &MetricCandidate, kind: BindingKind, cluster: &str, at: u64) -> Binding {
    Binding {
        id: format!("diagnostic:{:?}:{}", kind, c.pod.pod_uid),
        schema_version: 1,
        revision: 1,
        deployment: c.deployment.clone(),
        cluster: cluster.into(),
        namespace: c.namespace.clone(),
        native_sandbox_uid: c.native_uid.clone(),
        claim_uid: c.controller_uid.clone(),
        sandbox_id: String::new(),
        owner_subject: if c.unassigned_pool {
            "unassigned_pool"
        } else {
            "unattributed"
        }
        .into(),
        pod_name: Some(c.pod.pod_name.clone()),
        pod_uid: Some(c.pod.pod_uid.clone()),
        runtime_container_id: c.pod.runtime_container_id.clone(),
        restart_count: c.pod.runtime_restart_count,
        runtime_generation: None,
        pvc_name: c.pvc_name.clone(),
        pvc_uid: c.pvc_uid.clone(),
        kind,
        cpu_limit_cores: c.cpu_limit,
        memory_limit_bytes: c.memory_limit,
        valid_from: at,
        valid_until: None,
        observed_at: at,
        proof_version: 1,
        proof_status: ProofStatus::Verified,
    }
}
