//! Scoped admission owns desktop exclusion independently of the HTTP caller.
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use layer2_core::sandbox::store::{SandboxStoreError, WorkAuthority};
use layer2_core::sandbox::{try_acquire_runtime_migration_guard, RuntimeMigrationGuard};
use layer2_core::SandboxId;

const MAX_PENDING_ADMISSIONS: usize = 16;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
static REGISTRY: OnceLock<Arc<Registry>> = OnceLock::new();

struct Registry {
    limit: usize,
    guards: Mutex<BTreeMap<SandboxId, RuntimeMigrationGuard>>,
}

impl Registry {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            guards: Mutex::new(BTreeMap::new()),
        }
    }

    fn reserve(self: &Arc<Self>, id: &SandboxId) -> Result<Custody, SandboxStoreError> {
        let mut guards = self
            .guards
            .lock()
            .map_err(|_| SandboxStoreError::Conflict)?;
        // Capacity is reserved before polling any admission/driver work. An
        // unresolved entry cannot be evicted to make room for another request.
        if guards.len() >= self.limit || guards.contains_key(id) {
            return Err(SandboxStoreError::Conflict);
        }
        let guard =
            try_acquire_runtime_migration_guard(id).map_err(|_| SandboxStoreError::Conflict)?;
        guards.insert(id.clone(), guard);
        Ok(Custody {
            registry: self.clone(),
            id: id.clone(),
            commit_pending: false,
        })
    }
}

/// The registry, rather than the request or worker future, owns the guard.
/// A panic or unresolved return after commit dispatch leaves that ID closed.
pub(super) struct Custody {
    registry: Arc<Registry>,
    id: SandboxId,
    commit_pending: bool,
}

impl Custody {
    pub(super) fn commit_started(&mut self) {
        self.commit_pending = true;
    }

    /// Only a definite commit/abort or exact receipt reconciliation calls this.
    /// A missing timestamp, read failure or receipt mismatch is not an abort.
    pub(super) fn confirmed(&mut self) {
        self.commit_pending = false;
    }

    pub(super) async fn reconcile<F, Fut>(
        &mut self,
        snapshot_time: Option<mongodb::bson::Timestamp>,
        read: F,
    ) -> Result<(), SandboxStoreError>
    where
        F: FnOnce(mongodb::bson::Timestamp) -> Fut,
        Fut: Future<Output = Result<bool, SandboxStoreError>>,
    {
        let snapshot_time = snapshot_time.ok_or_else(|| {
            SandboxStoreError::Unavailable(
                "runtime migration commit unknown without an operation time".into(),
            )
        })?;
        if !read(snapshot_time).await? {
            return Err(SandboxStoreError::Unavailable(
                "runtime migration commit unknown: exact receipt not established".into(),
            ));
        }
        self.confirmed();
        Ok(())
    }
}

impl Drop for Custody {
    fn drop(&mut self) {
        if self.commit_pending {
            tracing::error!(sandbox_id = %self.id,
                "runtime migration finality unknown; retaining desktop exclusion");
            return;
        }
        if let Ok(mut guards) = self.registry.guards.lock() {
            guards.remove(&self.id);
        }
        // Poisoned custody also stays closed; never recover by discarding holds.
    }
}

pub(super) async fn run<T, F, Fut>(id: &SandboxId, operation: F) -> Result<T, SandboxStoreError>
where
    T: Send + 'static,
    F: FnOnce(Custody) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, SandboxStoreError>> + Send + 'static,
{
    run_with_registry(
        REGISTRY
            .get_or_init(|| Arc::new(Registry::new(MAX_PENDING_ADMISSIONS)))
            .clone(),
        id,
        RESPONSE_TIMEOUT,
        operation,
    )
    .await
}

async fn run_with_registry<T, F, Fut>(
    registry: Arc<Registry>,
    id: &SandboxId,
    response_timeout: Duration,
    operation: F,
) -> Result<T, SandboxStoreError>
where
    T: Send + 'static,
    F: FnOnce(Custody) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T, SandboxStoreError>> + Send + 'static,
{
    // This port is qualified for operator/sweeper admission in AM1/OFF, not
    // leased worker execution. Do not silently drop a caller's task-local lease.
    if WorkAuthority::active() {
        return Err(SandboxStoreError::Conflict);
    }
    let custody = registry.reserve(id)?;
    let mut task = tokio::spawn(crate::telemetry::agent_output::inherit_commands(
        async move { operation(custody).await },
    ));
    // Dropping a JoinHandle detaches; neither timeout nor caller cancellation
    // cancels the driver operation or drops the registry's desktop permit.
    match tokio::time::timeout(response_timeout, &mut task).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(SandboxStoreError::Unavailable(
            "runtime migration admission task terminated before finality".into(),
        )),
        Err(_) => Err(SandboxStoreError::Unavailable(
            "runtime migration admission exceeded its response deadline; custody retained".into(),
        )),
    }
}

#[cfg(test)]
mod native_probe;
#[cfg(test)]
mod tests;
