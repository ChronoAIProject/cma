use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::SandboxId;
use tokio::task::JoinHandle;

use crate::sandbox::worker::SandboxWorker;
use crate::sandbox::workspace_worker::WorkspaceWorker;

// Production logs run_one errors and rediscovers due work, including Unavailable:
// api/crates/cma/src/sandbox/agent_scheduler.rs:340 (retry tick at :142).
// Sandbox/workspace sweeps likewise continue after errors in
// api/crates/cma/src/sandbox/scheduler.rs:165 (next sweep at :174).
// Share 150 retries across all three workers: at the 20 ms cadence this allows
// 3 s of retry sleeps, within the unchanged 15 s / 60 s completion deadlines.
#[derive(Clone, Default)]
pub(super) struct StoreRetry {
    unavailable_retries: Arc<AtomicUsize>,
}

impl StoreRetry {
    pub(super) fn should_retry<T>(&self, result: Result<T, SandboxStoreError>) -> bool {
        match result {
            Err(SandboxStoreError::Unavailable(_)) => {
                let retries = self.unavailable_retries.fetch_add(1, Ordering::SeqCst) + 1;
                assert!(retries <= 150, "store unavailable retry budget exhausted");
                true
            }
            result => {
                result.unwrap();
                false
            }
        }
    }

    pub(super) fn spawn_environment(
        &self,
        sandbox_worker: Arc<SandboxWorker>,
        sandbox_id: SandboxId,
    ) -> JoinHandle<()> {
        let environment_retry = self.clone();
        tokio::spawn(async move {
            loop {
                environment_retry
                    .should_retry(sandbox_worker.run_one(&sandbox_id, SystemTime::now()).await);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    }

    pub(super) fn spawn_workspace(&self, workspace_worker: Arc<WorkspaceWorker>) -> JoinHandle<()> {
        let workspace_retry = self.clone();
        tokio::spawn(async move {
            loop {
                workspace_retry.should_retry(workspace_worker.run_due(SystemTime::now()).await);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    }
}
