use std::sync::atomic::{AtomicUsize, Ordering};

use layer2_core::sandbox::store::SandboxStoreError;

// Production logs run_one errors and rediscovers due work, including Unavailable:
// api/crates/cma/src/sandbox/agent_scheduler.rs:340 (retry tick at :142).
// Sandbox/workspace sweeps likewise continue after errors in
// api/crates/cma/src/sandbox/scheduler.rs:165 (next sweep at :174).
// Keep the test's shared retry budget bounded while it drives durable work.
#[derive(Default)]
pub(super) struct StoreRetry {
    unavailable_retries: AtomicUsize,
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
}
