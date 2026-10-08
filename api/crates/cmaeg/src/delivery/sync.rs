//! Process-local streaming waiters, with bounded frames and a shared byte budget.
use axum::response::sse::Event;
use futures_util::{stream, Stream};
use serde_json::Value;
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

struct Entry {
    sender: mpsc::Sender<Frame>,
    failed: Arc<AtomicBool>,
}
struct Frame {
    name: &'static str,
    data: String,
    terminal: bool,
    _permit: OwnedSemaphorePermit,
}
pub struct SyncRegistry {
    waiters: Mutex<HashMap<String, Entry>>,
    bytes: Arc<Semaphore>,
}
impl Default for SyncRegistry {
    fn default() -> Self {
        Self {
            waiters: Mutex::new(HashMap::new()),
            bytes: Arc::new(Semaphore::new(32 * 1024 * 1024)),
        }
    }
}
pub struct Waiter {
    id: String,
    registry: Arc<SyncRegistry>,
    receiver: mpsc::Receiver<Frame>,
    failed: Arc<AtomicBool>,
    ended: bool,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        self.registry.waiters.lock().unwrap().remove(&self.id);
    }
}
impl SyncRegistry {
    pub fn register(self: &Arc<Self>) -> Option<(String, Waiter)> {
        let mut waiters = self.waiters.lock().unwrap();
        if waiters.len() >= 4096 {
            return None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(8);
        let failed = Arc::new(AtomicBool::new(false));
        waiters.insert(
            id.clone(),
            Entry {
                sender,
                failed: failed.clone(),
            },
        );
        Some((
            id.clone(),
            Waiter {
                id,
                registry: self.clone(),
                receiver,
                failed,
                ended: false,
            },
        ))
    }
    pub fn emit(&self, waiter: &str, name: &'static str, data: &Value, terminal: bool) -> bool {
        let mut waiters = self.waiters.lock().unwrap();
        let Some(entry) = waiters.get(waiter) else {
            return false;
        };
        let data = data.to_string();
        let permit = self
            .bytes
            .clone()
            .try_acquire_many_owned(data.len().min(u32::MAX as usize) as u32);
        let sent = match permit {
            Ok(permit) if data.len() <= 1048576 => entry
                .sender
                .try_send(Frame {
                    name,
                    data,
                    terminal,
                    _permit: permit,
                })
                .is_ok(),
            _ => false,
        };
        if !sent {
            entry.failed.store(true, Ordering::Release);
        }
        if terminal || !sent {
            waiters.remove(waiter);
        }
        sent
    }
    /// Whether `waiter` can still take frames. A terminal frame, a failed
    /// emit or a disconnected client removes it, so a later reply cannot
    /// reach it.
    pub fn registered(&self, waiter: &str) -> bool {
        self.waiters.lock().unwrap().contains_key(waiter)
    }
    pub fn len(&self) -> usize {
        self.waiters.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
impl Waiter {
    pub fn stream(self) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
        stream::unfold(self, |mut waiter| async move {
            if waiter.ended {
                return None;
            }
            let frame = if waiter.failed.load(Ordering::Acquire) {
                None
            } else {
                waiter.receiver.recv().await
            };
            if waiter.failed.load(Ordering::Acquire) {
                waiter.ended = true;
                let event = Event::default()
                    .event("cmaeg.failed")
                    .data(r#"{"error":{"code":"internal","detail":"sync_backpressure"}}"#);
                return Some((Ok(event), waiter));
            }
            let frame = frame?;
            waiter.ended = frame.terminal;
            Some((
                Ok(Event::default().event(frame.name).data(frame.data)),
                waiter,
            ))
        })
    }
}
