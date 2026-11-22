use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{Notify, Semaphore};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Point {
    BeforeGuard,
    AfterGuard,
    AfterCommit,
}

pub(super) struct Pause {
    point: Point,
    armed: AtomicBool,
    reached: Notify,
    release: Semaphore,
}

impl Pause {
    pub(super) fn new(point: Point) -> Arc<Self> {
        Arc::new(Self {
            point,
            armed: AtomicBool::new(true),
            reached: Notify::new(),
            release: Semaphore::new(0),
        })
    }

    pub(super) async fn hit(&self, point: Point) {
        if self.point == point && self.armed.swap(false, Ordering::SeqCst) {
            self.reached.notify_one();
            tokio::time::timeout(std::time::Duration::from_secs(30), self.release.acquire())
                .await
                .expect("test must release paused transaction")
                .unwrap()
                .forget();
        }
    }

    pub(super) async fn reached(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(15), self.reached.notified())
            .await
            .expect("transaction must reach test pause");
    }

    pub(super) fn release(&self) {
        self.release.add_permits(1);
    }
}

impl super::MongoAccountDeletionStore {
    pub(super) async fn pause_at(&self, point: Point) {
        if let Some(pause) = &self.commit_pause {
            pause.hit(point).await;
        }
    }
}
