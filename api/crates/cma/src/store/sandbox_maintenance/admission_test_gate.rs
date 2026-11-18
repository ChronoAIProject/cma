//! Pause one selected admission after its real candidate-local writes.
use std::{cell::RefCell, future::Future, sync::Arc};
use tokio::sync::Notify;

tokio::task_local! {
    static LOCAL_WRITES_GATE: RefCell<Option<Arc<Gate>>>;
}

#[derive(Default)]
pub(super) struct Gate {
    reached: Notify,
    released: Notify,
}

impl Gate {
    pub(super) async fn scope<F: Future>(self: Arc<Self>, future: F) -> F::Output {
        LOCAL_WRITES_GATE
            .scope(RefCell::new(Some(self)), future)
            .await
    }

    pub(super) async fn reached(&self) {
        self.reached.notified().await;
    }

    pub(super) fn release(&self) {
        self.released.notify_one();
    }
}

pub(super) async fn after_local_writes() {
    let gate = LOCAL_WRITES_GATE
        .try_with(|gate| gate.borrow_mut().take())
        .ok()
        .flatten();
    if let Some(gate) = gate {
        gate.reached.notify_one();
        gate.released.notified().await;
    }
}
