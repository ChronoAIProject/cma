//! Background reporting, isolated from Agent/lifecycle transactions.
mod allocation_maintenance;
mod diagnostics;
mod observer;
mod projection;
mod report;
mod report_trace;
mod source;
mod summary;
mod warmer;
use crate::{config::Config, store::usage_reports::ReportingStore};
use layer2_core::sandbox_metrics::{MetricsError, MetricsService};
use layer3_adapter::{
    agent_sandbox::{resolve_kubernetes_connection, AgentSandboxClient},
    sandbox_metrics::{MetricsClient, MetricsTransport},
};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub(crate) struct MetricsWorkers {
    store: Arc<ReportingStore>,
    source: Option<MetricsClient>,
    native: Option<AgentSandboxClient>,
    cluster: String,
    enabled: bool,
}
impl MetricsWorkers {
    pub(crate) async fn new(
        config: &Config,
        capacity: Arc<layer3_adapter::agent_sandbox::RuntimeCapacityAuthority>,
    ) -> Result<Self, String> {
        let key = config
            .auth_encryption_key
            .metrics_cursor_key(&config.deployment_id);
        let store = ReportingStore::connect_with_budgets(
            &config.mongodb_uri,
            &config.mongodb_database,
            &key,
            config.usage_metrics.budgets.clone(),
            config.usage_metrics.reporting_pool_size,
        )
        .await
        .map_err(|e| e.to_string())?;
        let store = store.with_view_policy(config.usage_metrics.views.clone());
        store
            .initialize_allocation_buckets(store.view_policy.allocation_rollups_enabled)
            .await
            .map_err(|e| e.to_string())?;
        let enabled = config.usage_metrics.enabled;
        let (source, native) = if enabled {
            let source = if let Some(url) = &config.usage_metrics.source_url {
                MetricsClient::internal(url).map_err(|e| e.to_string())?
            } else {
                MetricsClient::new(MetricsTransport::KubernetesProxy(
                    resolve_kubernetes_connection(None)
                        .await
                        .map_err(|e| e.to_string())?
                        .client,
                ))
            };
            let native_config = config
                .runtime
                .agent_sandbox
                .clone()
                .ok_or("usage metrics requires the native Sandbox observer configuration")?;
            let connection =
                resolve_kubernetes_connection(config.runtime.agent_sandbox_connection.as_ref())
                    .await
                    .map_err(|e| e.to_string())?;
            let native = AgentSandboxClient::new_with_authority(
                connection.client,
                native_config,
                &connection.authority,
                capacity,
            )
            .map_err(|e| e.to_string())?;
            (Some(source), Some(native))
        } else {
            (None, None)
        };
        Ok(Self {
            store,
            source,
            native,
            cluster: config.usage_metrics.cluster.clone(),
            enabled,
        })
    }
    pub(crate) fn router(&self) -> axum::Router {
        layer1_public::rest::sandbox_metrics_router(MetricsService::new(self.store.clone()))
    }
    pub(crate) fn spawn(self, shutdown: &mut crate::shutdown::ShutdownCoordinator) {
        if self.store.view_policy.allocation_rollups_enabled {
            let worker = self.clone();
            let signal = shutdown.subscribe();
            shutdown.push(tokio::spawn(async move {
                tokio::select! { _ = crate::shutdown::signalled(signal) => {},
                _ = worker.maintain_allocation_history() => {} }
            }));
        }
        for slot in 0..self.store.budgets.workers {
            let worker = self.clone();
            let signal = shutdown.subscribe();
            shutdown.push(tokio::spawn(async move {
                tokio::select!{_ = crate::shutdown::signalled(signal)=>{},_ = layer2_core::diagnostic_context::DiagnosticContext::background("metrics").scope(worker.reports(slot))=>{}}
            }));
        }
        if self.store.view_policy.enabled && self.store.view_policy.warm_enabled {
            let worker = self.clone();
            let signal = shutdown.subscribe();
            shutdown.push(tokio::spawn(async move {
                tokio::select! { _ = crate::shutdown::signalled(signal) => {}, _ = worker.warm_views() => {} }
            }));
        }
        if self.enabled {
            for task in ["observer", "rollups", "replay", "diagnostics"] {
                let worker = self.clone();
                let signal = shutdown.subscribe();
                shutdown.push(tokio::spawn(async move {
                    tokio::select!{_ = crate::shutdown::signalled(signal)=>{},_ = layer2_core::diagnostic_context::DiagnosticContext::background("metrics").with_phase(task).scope(worker.background(task))=>{}}
                }));
            }
        }
    }
    async fn background(&self, task: &str) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            // A delayed cycle must never inherit a later cycle's authority.
            let holder = format!("metrics-{task}-{}", uuid::Uuid::new_v4().simple());
            match self.store.lease(task, &holder, 70).await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    tracing::warn!(task, %error, "sandbox metrics lease unavailable");
                    continue;
                }
            }
            let start = std::time::Instant::now();
            let result = tokio::time::timeout(Duration::from_secs(60), async {
                match task {
                    "observer" => self.observe(&holder).await,
                    "rollups" => self.collect(&holder, false).await,
                    "replay" => self.collect(&holder, true).await,
                    _ => self.diagnostics(&holder).await,
                }
            })
            .await
            .unwrap_or(Err(MetricsError::Unavailable));
            tracing::info!(
                task,
                elapsed_ms = start.elapsed().as_millis() as u64,
                success = result.is_ok(),
                "sandbox metrics background cycle"
            );
            if let Err(error) = result {
                tracing::warn!(task,error=%error,"sandbox metrics cycle incomplete");
            }
            if let Err(error) = self.store.release(task, &holder).await {
                tracing::warn!(task, %error, "sandbox metrics lease release unconfirmed");
            }
        }
    }
}

#[cfg(test)]
pub(crate) async fn reports_fixture(store: Arc<ReportingStore>) {
    MetricsWorkers {
        store,
        source: None,
        native: None,
        cluster: "fixture".into(),
        enabled: false,
    }
    .reports(0)
    .await;
}

#[cfg(test)]
pub(crate) async fn generate_fixture(
    store: Arc<ReportingStore>,
    lease: &crate::store::usage_reports::ReportLease,
) -> layer2_core::sandbox_metrics::Result<layer2_core::sandbox_metrics::Report> {
    generate_fixture_with_collection(store, lease, true).await
}

#[cfg(test)]
pub(crate) async fn generate_fixture_with_collection(
    store: Arc<ReportingStore>,
    lease: &crate::store::usage_reports::ReportLease,
    enabled: bool,
) -> layer2_core::sandbox_metrics::Result<layer2_core::sandbox_metrics::Report> {
    let worker = MetricsWorkers {
        store,
        source: None,
        native: None,
        cluster: "fixture".into(),
        enabled,
    };
    tokio::time::timeout(Duration::from_secs(60), worker.generate(lease))
        .await
        .map_err(|_| MetricsError::Unavailable)?
}

#[cfg(test)]
pub(crate) use source::collect_fixture;
#[cfg(test)]
pub(crate) use warmer::warm_fixture;
