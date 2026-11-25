//! Local desktop exclusion and history for explicitly qualified sole-process admission.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use uuid::Uuid;

use crate::SandboxId;

const TRACKER_LIMIT: usize = 4096;
static TRACKERS: OnceLock<Mutex<Trackers>> = OnceLock::new();
static PROCESS: OnceLock<String> = OnceLock::new();

#[derive(Default)]
struct Trackers(BTreeMap<SandboxId, Arc<Tracker>>);

struct Tracker {
    gate: Arc<RwLock<()>>,
    activity: Mutex<Option<RuntimeDesktopActivitySnapshot>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeDesktopActivitySnapshot {
    pub process_incarnation: String,
    pub tracking_incarnation: String,
    pub activity_epoch: u64,
    pub opening_or_open: u32,
}

#[derive(Debug, thiserror::Error)]
#[error("runtime migration conflicts with local desktop activity or unavailable tracking")]
pub struct RuntimeMigrationBusy;

/// Excludes native opens until the migration transaction and acknowledgement
/// reconciliation finish. The caller must separately prove sole AM1/OFF scope;
/// this guard conveys no authority over another process or a runtime itself.
pub struct RuntimeMigrationGuard {
    _permit: OwnedRwLockWriteGuard<()>,
    _tracker: Arc<Tracker>,
}

pub(super) struct DesktopRuntimeGuard {
    _permit: OwnedRwLockReadGuard<()>,
    tracker: Arc<Tracker>,
}

pub fn runtime_desktop_process_incarnation() -> &'static str {
    PROCESS.get_or_init(|| Uuid::new_v4().to_string())
}

impl Trackers {
    fn get(&mut self, id: &SandboxId, limit: usize) -> Result<Arc<Tracker>, RuntimeMigrationBusy> {
        if let Some(tracker) = self.0.get(id) {
            return Ok(tracker.clone());
        }
        if self.0.len() >= limit {
            // Every acquisition/getter retains an Arc before releasing this
            // registry lock. An in-flight or open connection cannot be evicted.
            let idle = self
                .0
                .iter()
                .find(|(_, tracker)| Arc::strong_count(tracker) == 1)
                .map(|(id, _)| id.clone())
                .ok_or(RuntimeMigrationBusy)?;
            self.0.remove(&idle);
        }
        let tracker = Arc::new(Tracker {
            gate: Arc::new(RwLock::new(())),
            activity: Mutex::new(Some(RuntimeDesktopActivitySnapshot {
                process_incarnation: runtime_desktop_process_incarnation().to_owned(),
                tracking_incarnation: Uuid::new_v4().to_string(),
                activity_epoch: 0,
                opening_or_open: 0,
            })),
        });
        self.0.insert(id.clone(), tracker.clone());
        Ok(tracker)
    }
}

fn tracker(id: &SandboxId) -> Result<Arc<Tracker>, RuntimeMigrationBusy> {
    TRACKERS
        .get_or_init(|| Mutex::new(Trackers::default()))
        .lock()
        .map_err(|_| RuntimeMigrationBusy)?
        .get(id, TRACKER_LIMIT)
}

/// A first read creates a new baseline, never evidence of earlier quiet time.
/// This metadata read is safe while the caller holds an exclusive migration guard.
pub fn runtime_desktop_activity_snapshot(
    id: &SandboxId,
) -> Result<RuntimeDesktopActivitySnapshot, RuntimeMigrationBusy> {
    tracker(id)?
        .activity
        .lock()
        .map_err(|_| RuntimeMigrationBusy)?
        .clone()
        .ok_or(RuntimeMigrationBusy)
}

impl Tracker {
    fn advance(&self, count_change: i32) -> Result<(), RuntimeMigrationBusy> {
        let mut activity = self.activity.lock().map_err(|_| RuntimeMigrationBusy)?;
        let Some(mut next) = activity.take() else {
            return Err(RuntimeMigrationBusy);
        };
        // Overflow permanently invalidates this incarnation, including when it
        // happens during Drop. Never silently wrap an apparently quiet counter.
        next.activity_epoch = next
            .activity_epoch
            .checked_add(1)
            .ok_or(RuntimeMigrationBusy)?;
        next.opening_or_open = next
            .opening_or_open
            .checked_add_signed(count_change)
            .ok_or(RuntimeMigrationBusy)?;
        *activity = Some(next);
        Ok(())
    }
}

/// Busy means refuse and reobserve; never infer an empty fleet from this result.
pub fn try_acquire_runtime_migration_guard(
    id: &SandboxId,
) -> Result<RuntimeMigrationGuard, RuntimeMigrationBusy> {
    let tracker = tracker(id)?;
    let permit = tracker
        .gate
        .clone()
        .try_write_owned()
        .map_err(|_| RuntimeMigrationBusy)?;
    if tracker
        .activity
        .lock()
        .map_err(|_| RuntimeMigrationBusy)?
        .is_none()
    {
        return Err(RuntimeMigrationBusy);
    }
    Ok(RuntimeMigrationGuard {
        _permit: permit,
        _tracker: tracker,
    })
}

pub(super) fn try_acquire_desktop_runtime_guard(
    id: &SandboxId,
) -> Result<DesktopRuntimeGuard, RuntimeMigrationBusy> {
    let tracker = tracker(id)?;
    let permit = tracker
        .gate
        .clone()
        .try_read_owned()
        .map_err(|_| RuntimeMigrationBusy)?;
    tracker.advance(1)?;
    Ok(DesktopRuntimeGuard {
        _permit: permit,
        tracker,
    })
}

impl DesktopRuntimeGuard {
    pub(super) fn opened(&self) -> Result<(), RuntimeMigrationBusy> {
        self.tracker.advance(0)
    }
}

impl Drop for DesktopRuntimeGuard {
    fn drop(&mut self) {
        // Metadata changes before releasing the shared permit, so exclusive
        // admission cannot pass between socket destruction and its final epoch.
        let _ = self.tracker.advance(-1);
    }
}

#[cfg(test)]
#[path = "runtime_migration_guard_tests.rs"]
mod tests;
