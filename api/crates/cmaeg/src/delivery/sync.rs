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
    deadline: Arc<Mutex<Option<tokio::time::Instant>>>,
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
    deadline: Arc<Mutex<Option<tokio::time::Instant>>>,
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
        let deadline = Arc::new(Mutex::new(None));
        waiters.insert(
            id.clone(),
            Entry {
                sender,
                failed: failed.clone(),
                deadline: deadline.clone(),
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
                deadline,
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
    /// Bounds only explicitly opted-in continuation streams. It never extends
    /// a prior deadline or changes the terminal contract of ordinary streams.
    pub(crate) fn bound(&self, waiter: &str, budget: std::time::Duration) {
        if let Some(entry) = self.waiters.lock().unwrap().get(waiter) {
            let mut deadline = entry.deadline.lock().unwrap();
            deadline.get_or_insert_with(|| tokio::time::Instant::now() + budget);
        }
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
                let deadline = *waiter.deadline.lock().unwrap();
                match deadline {
                    Some(deadline) => {
                        match tokio::time::timeout_at(deadline, waiter.receiver.recv()).await {
                            Ok(frame) => frame,
                            Err(_) => {
                                waiter.ended = true;
                                let event = Event::default()
                                    .event("cmaeg.media.pending")
                                    .data(r#"{"status":"pending","retrieval":"channel_activity"}"#);
                                return Some((Ok(event), waiter));
                            }
                        }
                    }
                    None => waiter.receiver.recv().await,
                }
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
mod tests {
    use super::*;
    use axum::response::{IntoResponse, Sse};
    use tokio::time::{advance, Duration, Instant};

    #[tokio::test(start_paused = true)]
    async fn media_stream_deadline_emits_pending_and_cannot_be_extended() {
        let registry = Arc::new(SyncRegistry::default());
        let (id, waiter) = registry.register().unwrap();
        let start = Instant::now();
        registry.bound(&id, Duration::from_secs(120));
        advance(Duration::from_secs(60)).await;
        registry.bound(&id, Duration::from_secs(120));
        let response = Sse::new(waiter.stream()).into_response();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(120));
        let stream = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(stream.contains("event: cmaeg.media.pending"), "{stream}");
        assert!(!stream.contains("cmaeg.completed"), "{stream}");
        assert!(registry.is_empty());
    }
}
