//! Observable retirement for detached adapter tasks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

#[derive(Clone, Default)]
pub(crate) struct TaskRetirement {
    inner: Arc<TaskRetirementInner>,
}

#[derive(Default)]
struct TaskRetirementInner {
    retired: AtomicBool,
    changed: Notify,
}

impl TaskRetirement {
    pub(crate) fn guard(&self) -> RetireTaskOnDrop {
        RetireTaskOnDrop(self.clone())
    }

    pub(crate) async fn wait(&self) {
        loop {
            let changed = self.inner.changed.notified();
            if self.inner.retired.load(Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.inner.retired.load(Ordering::Acquire)
    }

    fn finish(&self) {
        if !self.inner.retired.swap(true, Ordering::AcqRel) {
            self.inner.changed.notify_waiters();
        }
    }
}

pub(crate) struct RetireTaskOnDrop(TaskRetirement);

impl Drop for RetireTaskOnDrop {
    fn drop(&mut self) {
        self.0.finish();
    }
}
