//! Test-only barriers in the real worker transaction and bootstrap boundaries.
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Notify;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) enum Boundary {
    BeforeAdmission,
    Admitted,
    BeforeCommit,
    BeforeInteractionFenceAuthority,
}

type Key = (String, Boundary);
static ARMED: LazyLock<Mutex<HashMap<Key, Arc<Barrier>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Default)]
pub(crate) struct Barrier {
    pub(crate) reached: Notify,
    pub(crate) resume: Notify,
}

pub(crate) fn arm(database: &str, boundary: Boundary) -> Arc<Barrier> {
    let barrier = Arc::new(Barrier::default());
    assert!(ARMED
        .lock()
        .unwrap()
        .insert((database.to_owned(), boundary), barrier.clone())
        .is_none());
    barrier
}

pub(in crate::store::agent_worker_transaction) async fn stop(database: &str, boundary: Boundary) {
    let barrier = ARMED
        .lock()
        .unwrap()
        .remove(&(database.to_owned(), boundary));
    if let Some(barrier) = barrier {
        barrier.reached.notify_one();
        // The cleanup race uses the real legacy writer for each stop step.
        // Each step independently validates the entire retained prefix.
        let pause_budget = match boundary {
            Boundary::Admitted | Boundary::BeforeCommit => 180,
            Boundary::BeforeAdmission | Boundary::BeforeInteractionFenceAuthority => 45,
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(pause_budget),
            barrier.resume.notified(),
        )
        .await
        .unwrap();
    }
}
