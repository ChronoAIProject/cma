//! Deterministic synchronization for composed PTY interface tests.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, Weak};

use layer2_core::SessionId;
use tokio::sync::Semaphore;

type GateKey = (String, &'static str);

struct GateState {
    entered: Semaphore,
    release: Semaphore,
}

static OPERATION_GATES: LazyLock<Mutex<HashMap<GateKey, Weak<GateState>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Holds one named operation after socket receipt and before its core call.
///
/// Available only with the crate's `test-support` feature. The production
/// router never installs one, so release builds carry neither this type nor its
/// registry.
pub struct OperationGate {
    key: GateKey,
    state: Arc<GateState>,
}

impl OperationGate {
    /// Install the sole gate for this session and operation.
    pub fn install(id: &SessionId, operation: &'static str) -> Self {
        let key = (id.0.clone(), operation);
        let state = Arc::new(GateState {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let replaced = OPERATION_GATES
            .lock()
            .expect("the PTY operation-gate registry is usable")
            .insert(key.clone(), Arc::downgrade(&state));
        assert!(
            replaced.and_then(|gate| gate.upgrade()).is_none(),
            "only one PTY operation gate may be installed for {key:?}"
        );
        Self { key, state }
    }

    /// Wait until the server has removed the frame from the socket source.
    pub async fn wait_until_received(&self) {
        self.state
            .entered
            .acquire()
            .await
            .expect("the PTY receipt gate remains open")
            .forget();
    }

    /// Let the accepted operation proceed into core.
    pub fn release(&self) {
        self.state.release.add_permits(1);
    }
}

impl Drop for OperationGate {
    fn drop(&mut self) {
        // Never strand the socket task if a failing assertion unwinds the test
        // before its explicit release.
        self.state.release.add_permits(1);
        let mut gates = OPERATION_GATES
            .lock()
            .expect("the PTY operation-gate registry is usable");
        if gates
            .get(&self.key)
            .and_then(Weak::upgrade)
            .is_some_and(|state| Arc::ptr_eq(&state, &self.state))
        {
            gates.remove(&self.key);
        }
    }
}

pub(super) async fn operation_received(id: &SessionId, operation: &'static str) {
    let key = (id.0.clone(), operation);
    let state = OPERATION_GATES
        .lock()
        .expect("the PTY operation-gate registry is usable")
        .get(&key)
        .and_then(Weak::upgrade);
    let Some(state) = state else {
        return;
    };
    state.entered.add_permits(1);
    state
        .release
        .acquire()
        .await
        .expect("the PTY release gate remains open")
        .forget();
}
