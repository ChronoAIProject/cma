use super::*;
use layer2_core::sandbox_metrics::{Binding, BindingKind, ProofStatus, Result};
use layer3_adapter::{agent_sandbox::MetricIdentity, RuntimeHandle};
impl MetricsWorkers {
    pub(super) async fn observe(&self, holder: &str) -> Result<()> {
        let Some(native) = &self.native else {
            return Ok(());
        };
        use futures_util::{future::join_all, FutureExt};
        let mut progress = self.store.progress("observer").await?;
        let started = std::time::Instant::now();
        let mut verified = 0;
        let mut unattributed = 0;
        let mut max_proof_age_seconds = 0;
        'cycle: while started.elapsed() < Duration::from_secs(45) {
            let page = self
                .store
                .observation_page(progress.after.as_deref())
                .await?;
            if page.is_empty() {
                progress.after = None;
                self.store
                    .save_progress("observer", holder, &progress)
                    .await?;
                break;
            }
            // Checkpoint each eight-proof batch. Even a fully slow inventory
            // advances fairly before the cycle deadline instead of restarting
            // the same 200 records forever.
            for batch in page.chunks(8) {
                if started.elapsed() >= Duration::from_secs(45) {
                    break 'cycle;
                }
                let futures: Vec<_> = batch
                    .iter()
                    .cloned()
                    .map(|sandbox| {
                        let native = native.clone();
                        async move {
                            let home = sandbox
                                .lifecycle
                                .home
                                .as_ref()
                                .and_then(|h| h.attachment())?;
                            let handle = sandbox
                                .runtime
                                .as_ref()
                                .map(|r| RuntimeHandle(r.runtime_id.clone()));
                            let runtime = handle.as_ref().zip(
                                sandbox
                                    .runtime
                                    .as_ref()
                                    .and_then(|r| r.provision_generation),
                            );
                            let identity = native.metric_identity(&home, runtime).await.ok()?;
                            Some((sandbox, identity, crate::store::usage_reports::now()))
                        }
                        .boxed()
                    })
                    .collect();
                for observation in join_all(futures).await {
                    let Some((sandbox, identity, at)) = observation else {
                        unattributed += 1;
                        continue;
                    };
                    if !self.store.authority_unchanged(&sandbox).await? {
                        unattributed += 1;
                        continue;
                    }
                    let mut bindings = vec![binding(
                        &identity,
                        &sandbox,
                        &self.cluster,
                        BindingKind::Home,
                        at,
                    )];
                    if identity.runtime.is_some() {
                        bindings.push(binding(
                            &identity,
                            &sandbox,
                            &self.cluster,
                            BindingKind::Compute,
                            at,
                        ));
                    }
                    // Serialize mapping revision/CAS commits after concurrent I/O.
                    self.store
                        .reconcile_bindings_with_evidence(
                            holder,
                            sandbox.id.as_str(),
                            bindings,
                            at,
                            self.measured.as_ref().map(|_| &sandbox),
                        )
                        .await?;
                    max_proof_age_seconds = max_proof_age_seconds
                        .max(crate::store::usage_reports::now().saturating_sub(at));
                    verified += 1;
                }
                progress.after = batch.last().map(|s| s.id.to_string());
                self.store
                    .save_progress("observer", holder, &progress)
                    .await?;
            }
        }
        self.store
            .expire_missing_bindings(holder, crate::store::usage_reports::now())
            .await?;
        tracing::info!(
            verified,
            unattributed,
            max_proof_age_seconds,
            "sandbox metric identity reconciliation"
        );
        Ok(())
    }
}
fn binding(
    identity: &MetricIdentity,
    sandbox: &layer2_core::sandbox::Sandbox,
    cluster: &str,
    kind: BindingKind,
    at: u64,
) -> Binding {
    let runtime = if kind == BindingKind::Compute {
        identity.runtime.as_ref()
    } else {
        None
    };
    Binding {
        id: String::new(),
        schema_version: 1,
        revision: 1,
        deployment: identity.deployment.clone(),
        cluster: cluster.into(),
        namespace: identity.namespace.clone(),
        native_sandbox_uid: identity.native_sandbox_uid.clone(),
        claim_uid: identity.claim_uid.clone(),
        sandbox_id: sandbox.id.to_string(),
        owner_subject: sandbox.owner.to_string(),
        pod_name: runtime.map(|r| r.pod.pod_name.clone()),
        pod_uid: runtime.map(|r| r.pod.pod_uid.clone()),
        runtime_container_id: runtime.and_then(|r| r.pod.runtime_container_id.clone()),
        restart_count: runtime.and_then(|r| r.pod.runtime_restart_count),
        runtime_generation: runtime.map(|r| r.generation),
        pvc_name: identity.pvc_name.clone(),
        pvc_uid: identity.pvc_uid.clone(),
        kind,
        cpu_limit_cores: runtime.and_then(|r| r.cpu_limit_cores),
        memory_limit_bytes: runtime.and_then(|r| r.memory_limit_bytes),
        valid_from: at,
        valid_until: None,
        observed_at: at,
        proof_version: 1,
        proof_status: ProofStatus::Verified,
    }
}
