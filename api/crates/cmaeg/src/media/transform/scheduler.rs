//! The pod's one transform slot and the order sets take it in.
//!
//! A set (one keyframe set, or one audio transcode) holds the slot from its
//! first job through its last, so only one job directory and one job process
//! ever exist. A channel has at most one set in the global queue: its later
//! sets wait in that channel's FIFO and enter the global queue, in order, when
//! its current set finishes. They are never refused for waiting; the caller's
//! own budget bounds the wait, and the channel's two derive slots bound the
//! FIFO's length. Audio waiters are served before keyframe waiters because a
//! turn's preparation waits for transcripts; each class is FIFO.
//!
//! A set's deadline starts when it enters the global queue and includes its
//! wait for the slot. The state sits behind a `std` mutex that is never held
//! across an `.await`.
use cmaeg_core::ids::ChannelId;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::{
    sync::Notify,
    time::{timeout_at, Instant},
};

/// A keyframe set: one probe and up to six frame jobs.
pub const KEYFRAME_SET_WALL: Duration = Duration::from_secs(30);
/// An audio set: one transcode.
pub const AUDIO_SET_WALL: Duration = Duration::from_secs(20);

/// The kind of set, which decides its priority and its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetClass {
    Audio,
    Keyframes,
}

impl SetClass {
    /// How long a set may take from entering the global queue.
    pub fn wall(self) -> Duration {
        match self {
            Self::Audio => AUDIO_SET_WALL,
            Self::Keyframes => KEYFRAME_SET_WALL,
        }
    }
}

/// The set's deadline passed before it reached the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotTimeout;

/// Where one waiting set is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Behind an earlier set of its own channel; no deadline runs yet.
    Channel,
    /// In the global queue since `entered`.
    Queued { entered: Instant },
    /// Holding the slot; its deadline still runs from `entered`.
    Granted { entered: Instant },
}

struct Waiter {
    channel: ChannelId,
    class: SetClass,
    phase: Phase,
    wake: Arc<Notify>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    holder: Option<u64>,
    waiters: HashMap<u64, Waiter>,
    /// Every channel with a set queued globally or holding the slot, and the
    /// ids of that channel's later sets in arrival order.
    channels: HashMap<ChannelId, VecDeque<u64>>,
    audio: VecDeque<u64>,
    keyframes: VecDeque<u64>,
}

impl State {
    fn queue(&mut self, class: SetClass) -> &mut VecDeque<u64> {
        match class {
            SetClass::Audio => &mut self.audio,
            SetClass::Keyframes => &mut self.keyframes,
        }
    }

    /// Put `id` in its class's global queue now and wake it, so its deadline
    /// starts.
    fn enter_queue(&mut self, id: u64) {
        let Some(waiter) = self.waiters.get_mut(&id) else {
            return;
        };
        waiter.phase = Phase::Queued {
            entered: Instant::now(),
        };
        waiter.wake.notify_one();
        let class = waiter.class;
        self.queue(class).push_back(id);
    }

    /// Give a free slot to the first audio waiter, else the first keyframe
    /// waiter.
    fn dispatch(&mut self) {
        if self.holder.is_some() {
            return;
        }
        let Some(id) = self
            .audio
            .pop_front()
            .or_else(|| self.keyframes.pop_front())
        else {
            return;
        };
        let Some(waiter) = self.waiters.get_mut(&id) else {
            return;
        };
        if let Phase::Queued { entered } = waiter.phase {
            waiter.phase = Phase::Granted { entered };
        }
        self.holder = Some(id);
        waiter.wake.notify_one();
    }

    /// `channel`'s current set is over: its next set, if any, enters the
    /// global queue; otherwise the channel has nothing queued.
    fn advance_channel(&mut self, channel: &ChannelId) {
        let next = self.channels.get_mut(channel).and_then(VecDeque::pop_front);
        match next {
            Some(id) => self.enter_queue(id),
            None => {
                self.channels.remove(channel);
            }
        }
    }

    /// Remove `id` wherever it is, as when its set ends or its caller goes.
    fn leave(&mut self, id: u64) {
        let Some(waiter) = self.waiters.remove(&id) else {
            return;
        };
        match waiter.phase {
            Phase::Channel => {
                if let Some(behind) = self.channels.get_mut(&waiter.channel) {
                    behind.retain(|other| *other != id);
                }
            }
            Phase::Queued { .. } => {
                self.queue(waiter.class).retain(|other| *other != id);
                self.advance_channel(&waiter.channel);
            }
            Phase::Granted { .. } => {
                if self.holder == Some(id) {
                    self.holder = None;
                }
                self.advance_channel(&waiter.channel);
            }
        }
        self.dispatch();
    }
}

/// The pod-wide transform slot. Clones share it.
#[derive(Clone, Default)]
pub struct TransformScheduler {
    state: Arc<Mutex<State>>,
}

impl TransformScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Every critical section leaves the queues consistent before it can
        // panic, so a poisoned lock still guards valid state.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Wait for the slot for one set of `class` from `channel`. The set's
    /// deadline starts when it enters the global queue; a set that does not
    /// reach the slot by then fails with [`SlotTimeout`]. Dropping the future
    /// removes the waiter.
    pub async fn acquire(
        &self,
        channel: &ChannelId,
        class: SetClass,
    ) -> Result<Ticket, SlotTimeout> {
        let (id, wake) = self.enqueue(channel, class);
        let guard = Waiting {
            scheduler: self,
            id,
            armed: true,
        };
        loop {
            let phase = self.lock().waiters.get(&id).map(|waiter| waiter.phase);
            match phase {
                Some(Phase::Granted { entered }) => {
                    return Ok(guard.into_ticket(entered + class.wall()));
                }
                Some(Phase::Queued { entered }) => {
                    let deadline = entered + class.wall();
                    if timeout_at(deadline, wake.notified()).await.is_err() {
                        // The guard's drop gives the slot on if it was
                        // granted in the same instant.
                        return Err(SlotTimeout);
                    }
                }
                Some(Phase::Channel) => wake.notified().await,
                // Only this future's guard removes its entry, so it is
                // always present here; a missing entry cannot be granted.
                None => return Err(SlotTimeout),
            }
        }
    }

    fn enqueue(&self, channel: &ChannelId, class: SetClass) -> (u64, Arc<Notify>) {
        let mut state = self.lock();
        let id = state.next_id;
        state.next_id += 1;
        let wake = Arc::new(Notify::new());
        state.waiters.insert(
            id,
            Waiter {
                channel: channel.clone(),
                class,
                phase: Phase::Channel,
                wake: wake.clone(),
            },
        );
        match state.channels.get_mut(channel) {
            Some(behind) => behind.push_back(id),
            None => {
                state.channels.insert(channel.clone(), VecDeque::new());
                state.enter_queue(id);
                state.dispatch();
            }
        }
        (id, wake)
    }

    /// Sets waiting in the global queue and in channel FIFOs, and whether
    /// the slot is held; for tests and diagnostics.
    pub fn snapshot(&self) -> SchedulerSnapshot {
        let state = self.lock();
        SchedulerSnapshot {
            held: state.holder.is_some(),
            queued: state.audio.len() + state.keyframes.len(),
            behind_channels: state.channels.values().map(VecDeque::len).sum(),
        }
    }
}

/// Counts of [`TransformScheduler`]'s queues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerSnapshot {
    pub held: bool,
    pub queued: usize,
    pub behind_channels: usize,
}

/// A waiter whose caller may still go away; dropped while armed, it leaves
/// the queues, giving the slot on if it was already granted.
struct Waiting<'a> {
    scheduler: &'a TransformScheduler,
    id: u64,
    armed: bool,
}

impl Waiting<'_> {
    fn into_ticket(mut self, deadline: Instant) -> Ticket {
        self.armed = false;
        Ticket {
            scheduler: self.scheduler.clone(),
            id: self.id,
            deadline,
        }
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.scheduler.lock().leave(self.id);
        }
    }
}

/// The slot, held by one set until dropped.
pub struct Ticket {
    scheduler: TransformScheduler,
    id: u64,
    deadline: Instant,
}

impl Ticket {
    /// When the set must stop: its entry into the global queue plus its
    /// class's wall.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.scheduler.lock().leave(self.id);
    }
}
