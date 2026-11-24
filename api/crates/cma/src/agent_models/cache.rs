//! Cache discovery data, never caller authority or credential material.
use super::*;
use layer1_public::rest::AgentModelRuntimeSupport;
use layer2_core::sandbox::RuntimeCapacityError;
use layer3_adapter::{RuntimeHandle, RuntimeStatus, SandboxAdapter};
use moka::sync::Cache;
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

const CAPACITY: u64 = 256;
type Entries<T> = Cache<[u8; 32], Arc<OnceCell<T>>>;

fn entries<T: Send + Sync + 'static>(ttl: Duration) -> Entries<T> {
    Cache::builder()
        .max_capacity(CAPACITY)
        .time_to_live(ttl)
        .build()
}
fn digest(value: &impl serde::Serialize) -> Result<[u8; 32], AgentModelCatalogError> {
    serde_json::to_vec(value)
        .map(|bytes| Sha256::digest(bytes).into())
        .map_err(|_| unavailable(Dependency::Local, Reason::InvalidEvidence))
}

pub(super) struct CatalogCache(Entries<Vec<AgentModelView>>);
impl CatalogCache {
    pub fn new() -> Self {
        Self(entries(Duration::from_secs(30)))
    }
    pub async fn get<F, Fut>(
        &self,
        sandbox: &Sandbox,
        service: &str,
        fetch: F,
    ) -> Result<Vec<AgentModelView>, AgentModelCatalogError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<AgentModelView>, AgentModelCatalogError>>,
    {
        // Each caller has already validated current material against this exact
        // authority. No bearer or credential material survives in the cache.
        let key = digest(&(
            &sandbox.id,
            &sandbox.created_at,
            &sandbox.owner,
            &sandbox.credential,
            service,
        ))?;
        let cell = self.0.get_with(key, || Arc::new(OnceCell::new()));
        metrics::counter!("cma_agent_discovery_total", "kind" => "models", "outcome" => if cell.initialized() { "hit" } else { "miss" }).increment(1);
        // Include time waiting for another initializer; a series of failed
        // fetches must not multiply the gateway's per-request IO bound.
        tokio::time::timeout(Duration::from_secs(15), cell.get_or_try_init(fetch))
            .await
            .map_err(|_| unavailable(Dependency::ModelGateway, Reason::Timeout))?
            .cloned()
    }
}

pub(crate) struct CachedModelSupport {
    source: Arc<dyn SandboxAdapter>,
    entries: Entries<RuntimeStatus>,
}

enum SupportProbeError {
    Unavailable,
    Capacity(RuntimeCapacityError),
}

impl CachedModelSupport {
    pub(crate) fn wrap(
        source: Arc<dyn SandboxAdapter>,
        enabled: bool,
    ) -> Arc<dyn AgentModelRuntimeSupport> {
        if !enabled {
            return Arc::new(crate::projections::AdapterAgentModelSupport(source));
        }
        Arc::new(Self {
            source,
            entries: entries(Duration::from_secs(3)),
        })
    }
}
#[async_trait::async_trait]
impl AgentModelRuntimeSupport for CachedModelSupport {
    async fn supported(&self, sandbox: &Sandbox) -> bool {
        self.checked_supported(sandbox).await.unwrap_or(false)
    }

    async fn checked_supported(&self, sandbox: &Sandbox) -> Result<bool, RuntimeCapacityError> {
        const TIMEOUT: Duration = Duration::from_secs(5);
        if sandbox.validate().is_err() {
            return Ok(false);
        }
        let Some(binding) = &sandbox.runtime else {
            return Ok(false);
        };
        let read = || async {
            let started = tokio::time::Instant::now();
            let observed = tokio::time::timeout(
                TIMEOUT,
                self.source
                    .runtime_status(&RuntimeHandle::new(&binding.runtime_id)),
            )
            .await;
            let (outcome, result) = match observed {
                Err(_) => ("timeout", Err(SupportProbeError::Unavailable)),
                Ok(Err(error)) => match RuntimeCapacityError::from_adapter(&error) {
                    Some(capacity) => ("capacity", Err(SupportProbeError::Capacity(capacity))),
                    None => ("unavailable", Err(SupportProbeError::Unavailable)),
                },
                Ok(Ok(status)) if !sandbox.runtime_process_matches(&status).unwrap_or(false) => {
                    ("invalid", Err(SupportProbeError::Unavailable))
                }
                Ok(Ok(status)) => (
                    if status.agent_model_catalog {
                        "observed"
                    } else {
                        "unsupported"
                    },
                    Ok(status),
                ),
            };
            metrics::counter!("cma_agent_discovery_total", "kind" => "model_support", "outcome" => outcome).increment(1);
            metrics::histogram!("cma_agent_model_support_seconds", "outcome" => outcome)
                .record(started.elapsed().as_secs_f64());
            result
        };
        // An unknown boot cannot bind an observation across requests.
        let observation = if binding.boot_id.is_none() {
            read().await
        } else {
            let Ok(key) = digest(&(
                &sandbox.id,
                &sandbox.created_at,
                &sandbox.owner,
                &sandbox.runtime,
                &sandbox.lifecycle.home,
                &sandbox.credential,
            )) else {
                return Ok(false);
            };
            let cell = self.entries.get_with(key, || Arc::new(OnceCell::new()));
            metrics::counter!("cma_agent_discovery_total", "kind" => "model_support", "outcome" => if cell.initialized() { "hit" } else { "miss" }).increment(1);
            // Only successful observations fill the cell. Capacity can recover
            // before the TTL, so another request must be able to probe again.
            match tokio::time::timeout(TIMEOUT, cell.get_or_try_init(read)).await {
                Ok(result) => result.cloned(),
                Err(_) => Err(SupportProbeError::Unavailable),
            }
        };
        match observation {
            Ok(status) => Ok(status.agent_model_catalog
                && status.installation_ready
                && status.credential_ready
                && sandbox.runtime_process_matches(&status).unwrap_or(false)),
            Err(SupportProbeError::Unavailable) => Ok(false),
            Err(SupportProbeError::Capacity(capacity)) => Err(capacity),
        }
    }
}

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod tests;
