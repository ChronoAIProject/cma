use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use sha2::{Digest as _, Sha256};
use tokio::sync::{watch, Mutex};
use uuid::Uuid;

use super::BUFFER_BYTES;
use layer3_adapter::agent_capacity_recovery::RetainedOutputBoundary;

const MAX_RETAINED_FRAMES: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetainedFrame {
    pub cursor: u64,
    pub bytes: Arc<[u8]>,
}

impl RetainedFrame {
    fn retained_bytes(&self) -> usize {
        self.bytes.len() + std::mem::size_of::<Self>()
    }
}

#[derive(Default)]
struct Journal {
    held: VecDeque<RetainedFrame>,
    bytes: usize,
    last_cursor: u64,
    acknowledged: Option<(u64, [u8; 32])>,
    lost: usize,
    finished: bool,
}

impl Journal {
    fn acknowledged_through(&self) -> u64 {
        self.acknowledged.map_or(0, |(cursor, _)| cursor)
    }
}

/// Evidence from the recorder ACK, kept together with the stream identity and cursor.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RetainedOutputEvidence {
    pub boundary: RetainedOutputBoundary,
    pub acknowledged_payload_sha256: Option<[u8; 32]>,
}

pub(crate) struct OutputSnapshot {
    pub frames: Vec<RetainedFrame>,
    pub finished_and_acknowledged: bool,
}

/// Native output remains owned here until the API confirms durable recording.
/// A full journal backpressures native stdout until a durable ACK frees space.
/// Actual gaps permanently invalidate the stream.
pub(crate) struct OutputRetention {
    legacy: AtomicBool,
    stream_id: Uuid,
    journal: Mutex<Journal>,
    changed: watch::Sender<()>,
}

impl Default for OutputRetention {
    fn default() -> Self {
        Self {
            legacy: AtomicBool::new(false),
            stream_id: Uuid::new_v4(),
            journal: Mutex::new(Journal::default()),
            changed: watch::channel(()).0,
        }
    }
}

impl OutputRetention {
    pub(crate) fn stream_id(&self) -> Uuid {
        self.stream_id
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub(crate) fn select_legacy(&self) {
        self.legacy.store(true, Ordering::SeqCst);
        self.changed.send_replace(());
    }

    pub(crate) async fn retain(&self, bytes: &[u8]) {
        let mut changed = self.subscribe();
        let mut journal = loop {
            if self.legacy.load(Ordering::SeqCst) {
                return;
            }
            let journal = self.journal.lock().await;
            let size = bytes
                .len()
                .saturating_add(std::mem::size_of::<RetainedFrame>());
            if journal.lost > 0
                || size > BUFFER_BYTES
                || (journal.held.len() < MAX_RETAINED_FRAMES
                    && size <= BUFFER_BYTES.saturating_sub(journal.bytes))
            {
                break journal;
            }
            // Release the lock before waiting so the current attachment can ACK.
            // Subscribe before checking capacity to avoid a lost wakeup.
            drop(journal);
            let _ = changed.changed().await;
        };
        let Some(cursor) = journal.last_cursor.checked_add(1) else {
            journal.lost = journal.lost.saturating_add(1);
            self.changed.send_replace(());
            return;
        };
        if journal.lost > 0
            || journal.held.len() >= MAX_RETAINED_FRAMES
            || bytes.len() + std::mem::size_of::<RetainedFrame>()
                > BUFFER_BYTES.saturating_sub(journal.bytes)
        {
            journal.lost = journal.lost.saturating_add(1);
        } else {
            let retained = RetainedFrame {
                cursor,
                bytes: Arc::from(bytes),
            };
            journal.bytes += retained.retained_bytes();
            journal.held.push_back(retained);
        }
        journal.last_cursor = cursor;
        self.changed.send_replace(());
    }

    pub(crate) async fn gap(&self) {
        let mut journal = self.journal.lock().await;
        journal.last_cursor = journal.last_cursor.saturating_add(1);
        journal.lost = journal.lost.saturating_add(1);
        self.changed.send_replace(());
    }

    pub(crate) async fn finish(&self) {
        self.journal.lock().await.finished = true;
        self.changed.send_replace(());
    }

    pub(crate) async fn snapshot(&self, after: u64) -> Result<OutputSnapshot, usize> {
        let journal = self.journal.lock().await;
        if journal.lost > 0 {
            return Err(journal.lost);
        }
        Ok(OutputSnapshot {
            frames: journal
                .held
                .iter()
                .filter(|frame| frame.cursor > after)
                .cloned()
                .collect(),
            finished_and_acknowledged: journal.finished
                && journal.acknowledged_through() == journal.last_cursor,
        })
    }

    pub(crate) async fn recovery_evidence(&self) -> Option<RetainedOutputEvidence> {
        let journal = self.journal.lock().await;
        if self.legacy.load(Ordering::SeqCst) || journal.lost > 0 || journal.last_cursor == u64::MAX
        {
            return None;
        }
        let first_retained = journal.held.front().map_or_else(
            || journal.last_cursor.saturating_add(1),
            |frame| frame.cursor,
        );
        Some(RetainedOutputEvidence {
            boundary: RetainedOutputBoundary {
                stream_id: self.stream_id.to_string(),
                acknowledged_through: journal.acknowledged_through(),
                first_retained,
                observed_through: journal.last_cursor,
            },
            acknowledged_payload_sha256: journal.acknowledged.map(|(_, digest)| digest),
        })
    }

    pub(crate) async fn acknowledge(&self, stream_id: Uuid, cursor: u64) -> bool {
        if stream_id != self.stream_id || cursor == 0 {
            return false;
        }
        let mut journal = self.journal.lock().await;
        if self.legacy.load(Ordering::SeqCst) {
            return false;
        }
        let acknowledged = journal.acknowledged_through();
        if cursor <= acknowledged {
            return true;
        }
        let Ok(index) = usize::try_from(cursor - acknowledged - 1) else {
            return false;
        };
        let Some(frame) = journal
            .held
            .get(index)
            .filter(|frame| frame.cursor == cursor)
        else {
            return false;
        };
        // The wire prefix carries stream and cursor separately. Hash the same raw
        // ACP payload that the recorder associates with that position.
        let digest = Sha256::digest(&frame.bytes).into();
        while journal
            .held
            .front()
            .is_some_and(|frame| frame.cursor <= cursor)
        {
            let frame = journal.held.pop_front().expect("front checked above");
            journal.bytes -= frame.retained_bytes();
        }
        journal.acknowledged = Some((cursor, digest));
        self.changed.send_replace(());
        true
    }
}

#[cfg(test)]
#[path = "bridge_output_tests.rs"]
mod tests;
