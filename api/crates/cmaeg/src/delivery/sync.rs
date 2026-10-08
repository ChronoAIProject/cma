//! Process-local streaming waiters, with bounded frames and a shared byte budget.
mod fanout;

use crate::tools::ConsumerSummary;
use axum::response::sse::Event;
use cmaeg_core::activity::ConsumerTarget;
use fanout::{Emission, Fanout};
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
    fanout: Option<Fanout>,
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
        self.insert(8, None)
    }
    pub fn register_fanout(
        self: &Arc<Self>,
        consumers: u8,
        default: ConsumerSummary,
    ) -> Option<(String, Waiter)> {
        let fanout = Fanout::new(consumers, default)?;
        self.insert((8 * usize::from(consumers)).min(64), Some(fanout))
    }
    /// Pin routing order once, before any child can emit. Failure is an
    /// internal setup error; ingress drops its waiter instead of launching.
    pub fn set_fanout_targets(&self, waiter: &str, targets: &[ConsumerTarget]) -> bool {
        self.waiters
            .lock()
            .unwrap()
            .get_mut(waiter)
            .and_then(|entry| entry.fanout.as_mut())
            .is_some_and(|fanout| fanout.set_targets(targets))
    }
    fn insert(
        self: &Arc<Self>,
        capacity: usize,
        fanout: Option<Fanout>,
    ) -> Option<(String, Waiter)> {
        let mut waiters = self.waiters.lock().unwrap();
        if waiters.len() >= 4096 {
            return None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(capacity);
        let failed = Arc::new(AtomicBool::new(false));
        waiters.insert(
            id.clone(),
            Entry {
                sender,
                failed: failed.clone(),
                fanout,
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
        let Some(entry) = waiters.get_mut(waiter) else {
            return false;
        };
        let (sent, ended) = match entry.fanout.as_mut() {
            None => (self.push(entry, name, data, terminal), terminal),
            Some(fanout) => match fanout.translate(name, data, terminal) {
                Ok(Emission::Suppressed(delivered)) => return delivered,
                Ok(Emission::Frame {
                    name,
                    data,
                    final_data,
                }) => {
                    let ended = final_data.is_some();
                    let sent = self.push(entry, name, &data, false)
                        && final_data.as_ref().is_none_or(|data| {
                            self.push(entry, "cmaeg.fanout.completed", data, true)
                        });
                    (sent, ended)
                }
                Err(()) => {
                    tracing::warn!("multiplexed Sync frame has invalid consumer identity or state");
                    (false, true)
                }
            },
        };
        if !sent {
            entry.failed.store(true, Ordering::Release);
        }
        if ended || !sent {
            waiters.remove(waiter);
        }
        sent
    }
    fn push(&self, entry: &Entry, name: &'static str, data: &Value, terminal: bool) -> bool {
        let data = data.to_string();
        let permit = self
            .bytes
            .clone()
            .try_acquire_many_owned(data.len().min(u32::MAX as usize) as u32);
        match permit {
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
        }
    }
    /// Whether `waiter` can still take frames. A plain or aggregate terminal,
    /// failed emit or disconnected client removes it. A multiplexed waiter
    /// may still exist while refusing late frames for a finished consumer.
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

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
