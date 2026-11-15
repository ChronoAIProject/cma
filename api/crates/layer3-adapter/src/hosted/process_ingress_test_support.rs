//! Serialize contracts that reserve or compare the process-wide ingress pool.

use crate::ingress_budget::IngressBudget;
use std::time::Duration;
use tokio::sync::{Mutex, MutexGuard};

pub(super) async fn process_budget_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::const_new(());
    LOCK.lock().await
}

pub(super) async fn wait_for_process_budget(expected: usize) {
    // Retiring a transport aborts its reader/relay tasks. Keep the test lock
    // until those tasks have dropped their last bridge and returned its leases.
    tokio::time::timeout(Duration::from_secs(2), async {
        while IngressBudget::process().available_bytes() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("retired test bridges release the process ingress budget");
}
