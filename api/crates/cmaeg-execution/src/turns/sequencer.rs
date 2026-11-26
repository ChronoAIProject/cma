//! Best-effort launch order per conversation.
//!
//! Order follows the order in which turns reach `launch` after verification
//! and resolution, per pod. A media turn may spend time preparing before its
//! provider turn starts, so a following turn in the same conversation waits
//! on a fence until its predecessor's start has returned.
//!
//! It is best effort: there is no cross-replica ordering, and a predecessor
//! that exceeds the bound is passed. Entries exist only while a media chain
//! is live, so the turn tracker (4 096 live turns) bounds them; an
//! attachment-free turn with no live chain creates nothing.
use crate::channel_hash;
use cmaeg_core::ids::{ChannelId, ConversationId};
use dashmap::{mapref::entry::Entry, DashMap};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::watch;

pub const FENCE_WAIT: &str = "cmaeg_media_fence_wait_seconds";

pub type ChainKey = (ChannelId, ConversationId);

/// The live media chains of this pod, keyed by conversation.
#[derive(Default)]
pub struct Sequencer {
    chains: Arc<DashMap<ChainKey, Link>>,
    next: AtomicU64,
}

/// The newest turn of a chain; its successor waits on `released`.
struct Link {
    id: u64,
    released: watch::Receiver<bool>,
}

/// One turn's place in its conversation's chain.
///
/// Dropping a ticket releases it, so a turn that panics or times out never
/// holds its successor beyond the fence bound.
pub struct Ticket {
    chains: Arc<DashMap<ChainKey, Link>>,
    key: Option<ChainKey>,
    id: u64,
    own: Option<watch::Sender<bool>>,
    predecessor: Option<watch::Receiver<bool>>,
}

impl Sequencer {
    /// Join `key`'s chain. A turn joins when a chain is live, or starts one
    /// when `hold` is set; otherwise its ticket is inert and nothing is
    /// recorded. The entry API makes the read and the replacement atomic.
    pub fn enter(&self, key: ChainKey, hold: bool) -> Ticket {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let ticket = |key, own, predecessor| Ticket {
            chains: self.chains.clone(),
            key,
            id,
            own,
            predecessor,
        };
        match self.chains.entry(key) {
            Entry::Occupied(mut entry) => {
                let (own, released) = watch::channel(false);
                let key = entry.key().clone();
                let previous = entry.insert(Link { id, released });
                ticket(Some(key), Some(own), Some(previous.released))
            }
            Entry::Vacant(entry) if hold => {
                let (own, released) = watch::channel(false);
                let key = entry.key().clone();
                entry.insert(Link { id, released });
                ticket(Some(key), Some(own), None)
            }
            Entry::Vacant(_) => ticket(None, None, None),
        }
    }

    /// Conversations with a live chain.
    pub fn live_chains(&self) -> usize {
        self.chains.len()
    }
}

impl Ticket {
    /// Whether a predecessor must start first.
    pub fn waits(&self) -> bool {
        self.predecessor.is_some()
    }

    /// Wait at most `max` for the predecessor's release. `false` means the
    /// bound passed and the turn proceeds out of order.
    pub async fn wait(&mut self, max: Duration) -> bool {
        let Some(predecessor) = self.predecessor.as_mut() else {
            return true;
        };
        let started = Instant::now();
        // A dropped sender counts as released: its turn can no longer start.
        let released = tokio::time::timeout(max, predecessor.wait_for(|released| *released))
            .await
            .is_ok();
        metrics::histogram!(FENCE_WAIT).record(started.elapsed().as_secs_f64());
        if !released {
            let channel = self
                .key
                .as_ref()
                .map(|(channel, _)| channel_hash(channel.as_str()))
                .unwrap_or_default();
            tracing::warn!(target: "cmaeg::turns::sequencer", channel = %channel, "conversation order fence timed out");
        }
        released
    }

    /// Let the successor start. Idempotent: only the first call signals,
    /// and only the newest link removes the chain's entry.
    pub fn release(&mut self) {
        let Some(own) = self.own.take() else {
            return;
        };
        // No receiver means no successor is waiting.
        let _ = own.send(true);
        if let Some(key) = &self.key {
            let id = self.id;
            self.chains.remove_if(key, |_, link| link.id == id);
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.release()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(conversation: &str) -> ChainKey {
        (
            ChannelId::new("cmaeg1.ch.test").unwrap(),
            ConversationId::new(conversation).unwrap(),
        )
    }

    #[test]
    fn an_attachment_free_turn_without_a_chain_is_inert() {
        let sequencer = Sequencer::default();
        let mut ticket = sequencer.enter(key("a"), false);
        assert!(!ticket.waits());
        assert_eq!(sequencer.live_chains(), 0);
        ticket.release();
        assert_eq!(sequencer.live_chains(), 0);
    }

    #[tokio::test]
    async fn a_dropped_predecessor_releases_its_successor() {
        let sequencer = Sequencer::default();
        let first = sequencer.enter(key("a"), true);
        let mut second = sequencer.enter(key("a"), false);
        assert!(!first.waits());
        assert!(second.waits());
        // A panicking turn unwinds and drops its ticket.
        drop(first);
        assert!(second.wait(Duration::from_secs(1)).await);
        assert_eq!(
            sequencer.live_chains(),
            1,
            "the successor is the newest link"
        );
        drop(second);
        assert_eq!(sequencer.live_chains(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_gives_up_at_its_bound() {
        let sequencer = Sequencer::default();
        let _first = sequencer.enter(key("a"), true);
        let mut second = sequencer.enter(key("a"), true);
        let started = tokio::time::Instant::now();
        assert!(!second.wait(Duration::from_secs(7)).await);
        assert_eq!(started.elapsed(), Duration::from_secs(7));
    }

    #[test]
    fn release_keeps_a_newer_link() {
        let sequencer = Sequencer::default();
        let mut first = sequencer.enter(key("a"), true);
        let second = sequencer.enter(key("a"), false);
        first.release();
        first.release();
        assert_eq!(sequencer.live_chains(), 1);
        assert_eq!(
            sequencer.chains.get(&key("a")).map(|link| link.id),
            Some(second.id)
        );
        drop(second);
        assert_eq!(sequencer.live_chains(), 0);
    }

    #[test]
    fn different_conversations_form_separate_chains() {
        let sequencer = Sequencer::default();
        let _first = sequencer.enter(key("a"), true);
        let other = sequencer.enter(key("b"), false);
        assert!(!other.waits());
        assert_eq!(sequencer.live_chains(), 1);
    }

    #[test]
    fn concurrent_entries_on_one_key_form_a_single_chain() {
        let sequencer = Arc::new(Sequencer::default());
        let mut tickets: Vec<Ticket> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let sequencer = sequencer.clone();
                    scope.spawn(move || sequencer.enter(key("a"), true))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        assert_eq!(sequencer.live_chains(), 1);
        // Exactly one ticket started the chain, and each release lets at
        // most one successor go: 15 links over 16 tickets with one head
        // form a single line.
        assert_eq!(tickets.iter().filter(|ticket| !ticket.waits()).count(), 1);
        let released = |tickets: &[Ticket]| {
            tickets
                .iter()
                .filter(|ticket| {
                    ticket
                        .predecessor
                        .as_ref()
                        .is_some_and(|predecessor| *predecessor.borrow())
                })
                .count()
        };
        let mut previous = 0;
        for index in 0..tickets.len() {
            tickets[index].release();
            let now = released(&tickets);
            assert!(now - previous <= 1, "one release freed several turns");
            previous = now;
        }
        assert_eq!(previous, 15);
        assert_eq!(sequencer.live_chains(), 0);
    }
}
