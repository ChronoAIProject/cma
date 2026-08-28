//! Deterministic synchronization for composed PTY interface tests.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use layer2_core::SessionId;
use tokio::sync::Semaphore;

type GateKey = (String, &'static str);

const OUTBOUND_POINT: &str = "__outbound";
const SOURCE_CLOSED_POINT: &str = "__source_closed";

struct GateState {
    entered_once: AtomicBool,
    entered: Semaphore,
    release: Semaphore,
}

static BOUNDARY_GATES: LazyLock<Mutex<HashMap<GateKey, Weak<GateState>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Gate {
    key: GateKey,
    state: Arc<GateState>,
}

impl Gate {
    fn install(id: &SessionId, point: &'static str) -> Self {
        let key = (id.0.clone(), point);
        let state = Arc::new(GateState {
            entered_once: AtomicBool::new(false),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let replaced = BOUNDARY_GATES
            .lock()
            .expect("the PTY boundary-gate registry is usable")
            .insert(key.clone(), Arc::downgrade(&state));
        assert!(
            replaced.and_then(|gate| gate.upgrade()).is_none(),
            "only one PTY boundary gate may be installed for {key:?}"
        );
        Self { key, state }
    }

    async fn wait_until_entered(&self) {
        self.state
            .entered
            .acquire()
            .await
            .expect("the PTY boundary gate remains open")
            .forget();
    }

    fn release(&self) {
        self.state.release.add_permits(1);
    }

    fn was_entered(&self) -> bool {
        self.state.entered_once.load(Ordering::SeqCst)
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        // Never strand the socket task if a failing assertion unwinds the test
        // before its explicit release.
        self.state.release.add_permits(1);
        let mut gates = BOUNDARY_GATES
            .lock()
            .expect("the PTY boundary-gate registry is usable");
        if gates
            .get(&self.key)
            .and_then(Weak::upgrade)
            .is_some_and(|state| Arc::ptr_eq(&state, &self.state))
        {
            gates.remove(&self.key);
        }
    }
}

/// Holds one named operation after socket receipt and before its core call.
///
/// Available only with the crate's `test-support` feature. The production
/// router never installs one, so release builds carry neither this type nor its
/// registry.
pub struct OperationGate(Gate);

impl OperationGate {
    /// Install the sole gate for this session and operation.
    pub fn install(id: &SessionId, operation: &'static str) -> Self {
        Self(Gate::install(id, operation))
    }

    /// Wait until the server has removed the frame from the socket source.
    pub async fn wait_until_received(&self) {
        self.0.wait_until_entered().await;
    }

    /// Let the accepted operation proceed into core.
    pub fn release(&self) {
        self.0.release();
    }

    /// Whether the server ever removed this operation from the socket source.
    pub fn was_received(&self) -> bool {
        self.0.was_entered()
    }
}

/// Holds the writer after selecting terminal output and before sending it.
pub struct OutboundGate(Gate);

impl OutboundGate {
    /// Install the sole blocked-writer gate for this session.
    pub fn install(id: &SessionId) -> Self {
        Self(Gate::install(id, OUTBOUND_POINT))
    }

    /// Wait until the writer has selected a terminal output frame.
    pub async fn wait_until_blocked(&self) {
        self.0.wait_until_entered().await;
    }

    /// Let the writer attempt to send the selected output frame.
    pub fn release(&self) {
        self.0.release();
    }
}

/// Holds the input task after it observes the client connection ending.
pub struct DisconnectGate(Gate);

impl DisconnectGate {
    /// Install the sole disconnect-observation gate for this session.
    pub fn install(id: &SessionId) -> Self {
        Self(Gate::install(id, SOURCE_CLOSED_POINT))
    }

    /// Wait until the input task observes the socket ending.
    pub async fn wait_until_observed(&self) {
        self.0.wait_until_entered().await;
    }

    /// Let the input task finish after observing the disconnect.
    pub fn release(&self) {
        self.0.release();
    }
}

async fn wait_at(id: &SessionId, point: &'static str) {
    let key = (id.0.clone(), point);
    let state = BOUNDARY_GATES
        .lock()
        .expect("the PTY boundary-gate registry is usable")
        .get(&key)
        .and_then(Weak::upgrade);
    let Some(state) = state else {
        return;
    };
    state.entered_once.store(true, Ordering::SeqCst);
    state.entered.add_permits(1);
    state
        .release
        .acquire()
        .await
        .expect("the PTY boundary release gate remains open")
        .forget();
}

pub(super) async fn operation_received(id: &SessionId, operation: &'static str) {
    wait_at(id, operation).await;
}

pub(super) async fn outbound_blocked(id: &SessionId) {
    wait_at(id, OUTBOUND_POINT).await;
}

pub(super) async fn source_closed(id: &SessionId) {
    wait_at(id, SOURCE_CLOSED_POINT).await;
}
