//! Bounded replica-local route pacing; no lock is evicted while a send owns it.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{Mutex as AsyncMutex, OwnedMutexGuard},
    time::Instant,
};

#[derive(Default)]
pub struct Pacing {
    routes: Mutex<HashMap<String, Arc<AsyncMutex<Instant>>>>,
}
pub struct Slot(OwnedMutexGuard<Instant>);
impl Pacing {
    pub async fn acquire(&self, route: &str) -> Option<Slot> {
        let lock = {
            let mut routes = self.routes.lock().unwrap();
            if routes.len() >= 4096 {
                routes.retain(|_, lock| {
                    Arc::strong_count(lock) > 1
                        || lock.try_lock().map_or(true, |next| *next > Instant::now())
                });
            }
            if routes.len() >= 4096 && !routes.contains_key(route) {
                return None;
            }
            routes
                .entry(route.to_owned())
                .or_insert_with(|| Arc::new(AsyncMutex::new(Instant::now())))
                .clone()
        };
        let guard = lock.lock_owned().await;
        tokio::time::sleep_until(*guard).await;
        Some(Slot(guard))
    }
}
impl Slot {
    pub fn cooldown(&mut self, seconds: u64) {
        *self.0 = (*self.0).max(Instant::now() + Duration::from_secs(seconds.min(10)));
    }
    pub async fn retry(&mut self) {
        self.cooldown(3);
        tokio::time::sleep_until(*self.0).await;
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.cooldown(3);
    }
}
