//! Deterministic barriers after an identity decision or token consumption.
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

pub struct ResponseGate {
    entered: Semaphore,
    release: Semaphore,
    pub lose_response: bool,
}

impl ResponseGate {
    pub fn new(lose_response: bool) -> Arc<Self> {
        Arc::new(Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            lose_response,
        })
    }

    pub async fn hold(&self) {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }

    pub async fn reached(&self) {
        tokio::time::timeout(Duration::from_secs(10), self.entered.acquire())
            .await
            .expect("provider barrier reached")
            .unwrap()
            .forget();
    }

    pub fn release(&self) {
        self.release.add_permits(1);
    }
}
