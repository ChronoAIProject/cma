//! Bounded discovery and independent work for the durable channel pipeline.
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::watch,
    task::{Id, JoinHandle, JoinSet},
    time::MissedTickBehavior,
};

use super::operations::{elapsed_ms, WorkKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChannelWorkError {
    Unavailable,
    Changed,
    Unsupported,
    Suspended,
    Corrupt,
    AttachmentExpired,
    AttachmentUnavailable,
}

impl ChannelWorkError {
    pub(crate) fn retry_delay(self) -> Duration {
        match self {
            Self::Changed => Duration::from_millis(250),
            Self::Unavailable => Duration::from_secs(1),
            Self::AttachmentUnavailable => Duration::from_secs(5),
            Self::Unsupported | Self::Suspended | Self::Corrupt | Self::AttachmentExpired => {
                FAILURE_RETRY_DELAY
            }
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait ChannelWorkPort: Send + Sync {
    fn kind(&self) -> WorkKind;
    /// At most 32 ordered raw IDs. Per-row decoding belongs to run_one so one
    /// inconsistent document cannot block discovery of unrelated later work.
    async fn discover(&self, after: Option<&str>) -> Result<Vec<String>, ChannelWorkError>;
    async fn run_one(&self, id: &str) -> Result<(), ChannelWorkError>;
}

pub(crate) struct ChannelScheduler {
    port: Arc<dyn ChannelWorkPort>,
}

impl ChannelScheduler {
    pub(crate) fn new(port: Arc<dyn ChannelWorkPort>) -> Self {
        Self { port }
    }
    pub(crate) fn spawn(self, mut stop: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut state = Scheduled::new(self.port);
            let mut ticker = tokio::time::interval(Duration::from_millis(250));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                if *stop.borrow() {
                    break;
                }
                tokio::select! {
                    changed = stop.changed() => { if changed.is_err() || *stop.borrow() { break; } },
                    joined = state.tasks.join_next_with_id(), if !state.tasks.is_empty() => {
                        if let Some(joined) = joined { state.complete(joined); }
                    },
                    _ = ticker.tick() => state.poll(),
                }
                if !*stop.borrow() {
                    state.fill();
                }
            }
            if tokio::time::timeout(Duration::from_secs(25), async {
                while let Some(joined) = state.tasks.join_next_with_id().await {
                    state.complete(joined);
                }
            })
            .await
            .is_err()
            {
                state.tasks.shutdown().await;
            }
        })
    }
}

enum Kind {
    Discover,
    Work(String, tokio::time::Instant),
}
type ResultValue = Result<Option<Vec<String>>, ChannelWorkError>;
type Joined = Result<(Id, ResultValue), tokio::task::JoinError>;

const FAILURE_RETRY_DELAY: Duration = Duration::from_secs(30);
const MAX_RETRY_ENTRIES: usize = 4096;

struct Scheduled {
    port: Arc<dyn ChannelWorkPort>,
    tasks: JoinSet<ResultValue>,
    kinds: HashMap<Id, Kind>,
    active: BTreeSet<String>,
    queued: VecDeque<String>,
    cursor: Option<String>,
    discovering: bool,
    last_warning: Option<tokio::time::Instant>,
    last_observation: Option<tokio::time::Instant>,
    retry_after: HashMap<String, tokio::time::Instant>,
    discovery_after: Option<tokio::time::Instant>,
    capacity_after: Option<tokio::time::Instant>,
}
impl Scheduled {
    fn new(port: Arc<dyn ChannelWorkPort>) -> Self {
        Self {
            port,
            tasks: JoinSet::new(),
            kinds: HashMap::new(),
            active: BTreeSet::new(),
            queued: VecDeque::new(),
            cursor: None,
            discovering: false,
            last_warning: None,
            last_observation: None,
            retry_after: HashMap::new(),
            discovery_after: None,
            capacity_after: None,
        }
    }
    fn poll(&mut self) {
        let now = tokio::time::Instant::now();
        self.retry_after.retain(|_, deadline| *deadline > now);
        if self
            .last_observation
            .is_none_or(|prior| now - prior >= Duration::from_secs(30))
        {
            tracing::info!(
                target: "cma::bridge::operations",
                operation = "scheduler", worker = self.port.kind().label(),
                active = self.active.len(), queued = self.queued.len(),
                discovering = self.discovering,
                "channel_scheduler"
            );
            self.last_observation = Some(now);
        }
        if self.discovering
            || !self.queued.is_empty()
            || self.active.len() >= 4
            || self.discovery_after.is_some_and(|deadline| deadline > now)
            || self.capacity_after.is_some_and(|deadline| deadline > now)
        {
            return;
        }
        let port = self.port.clone();
        let after = self.cursor.clone();
        let handle = self.tasks.spawn(async move {
            let ids =
                tokio::time::timeout(Duration::from_secs(15), port.discover(after.as_deref()))
                    .await
                    .map_err(|_| ChannelWorkError::Unavailable)??;
            if ids.len() > 32
                || ids.iter().any(|id| id.is_empty())
                || ids.windows(2).any(|pair| pair[0] >= pair[1])
                || after
                    .as_ref()
                    .zip(ids.first())
                    .is_some_and(|(old, next)| old >= next)
            {
                return Err(ChannelWorkError::Corrupt);
            }
            Ok(Some(ids))
        });
        self.kinds.insert(handle.id(), Kind::Discover);
        self.discovering = true;
    }
    fn fill(&mut self) {
        let now = tokio::time::Instant::now();
        if self.capacity_after.is_some_and(|deadline| deadline > now) {
            return;
        }
        while self.active.len() < 4 {
            let Some(id) = self.queued.pop_front() else {
                break;
            };
            if self
                .retry_after
                .get(&id)
                .is_some_and(|deadline| *deadline > now)
                || !self.active.insert(id.clone())
            {
                continue;
            }
            let port = self.port.clone();
            let work_id = id.clone();
            let handle = self.tasks.spawn(async move {
                tokio::time::timeout(Duration::from_secs(60), port.run_one(&work_id))
                    .await
                    .map_err(|_| ChannelWorkError::Unavailable)??;
                Ok(None)
            });
            self.kinds
                .insert(handle.id(), Kind::Work(id, tokio::time::Instant::now()));
        }
    }
    fn complete(&mut self, joined: Joined) {
        let id = match &joined {
            Ok((id, _)) => *id,
            Err(error) => error.id(),
        };
        match self.kinds.remove(&id) {
            Some(Kind::Discover) => {
                self.discovering = false;
                self.discovery_after = match &joined {
                    Ok((_, Ok(Some(_)))) => None,
                    Ok((_, Err(error))) => Some(tokio::time::Instant::now() + error.retry_delay()),
                    _ => Some(tokio::time::Instant::now() + FAILURE_RETRY_DELAY),
                };
            }
            Some(Kind::Work(id, started)) => {
                self.active.remove(&id);
                // Discovery is frequent so new messages remain responsive. A
                // failed row must not reuse that cadence to hammer identity
                // providers or prevent their rate-limit windows recovering.
                if !matches!(&joined, Ok((_, Ok(_)))) {
                    let delay = match &joined {
                        Ok((_, Err(error))) => error.retry_delay(),
                        _ => FAILURE_RETRY_DELAY,
                    };
                    let deadline = tokio::time::Instant::now() + delay;
                    if self.retry_after.len() < MAX_RETRY_ENTRIES {
                        self.retry_after.insert(id.clone(), deadline);
                    } else {
                        // Bound process memory without dropping retry delays.
                        // An exceptional full window pauses this worker only.
                        self.capacity_after = Some(
                            self.capacity_after
                                .map_or(deadline, |prior| prior.max(deadline)),
                        );
                    }
                } else {
                    self.retry_after.remove(&id);
                }
                let outcome = match &joined {
                    Ok((_, Ok(_))) => "observed",
                    Ok((_, Err(ChannelWorkError::Changed))) => "changed",
                    Ok((_, Err(ChannelWorkError::Unsupported))) => "unsupported",
                    Ok((_, Err(ChannelWorkError::Suspended))) => "suspended",
                    Ok((_, Err(ChannelWorkError::Corrupt))) => "inconsistent",
                    Ok((_, Err(ChannelWorkError::Unavailable))) => "unavailable",
                    Ok((_, Err(ChannelWorkError::AttachmentExpired))) => "text_attachment_expired",
                    Ok((_, Err(ChannelWorkError::AttachmentUnavailable))) => {
                        "text_attachment_unavailable"
                    }
                    Err(_) => "worker_lost",
                };
                use sha2::{Digest, Sha256};
                let work_id_sha256 = format!("{:x}", Sha256::digest(id.as_bytes()));
                tracing::info!(
                    target: "cma::bridge::operations",
                    operation = "worker", worker = self.port.kind().label(), outcome,
                    elapsed_ms = elapsed_ms(started.elapsed()), work_id_sha256,
                    "channel_operation"
                );
            }
            None => {}
        }
        match joined {
            Ok((_, Ok(Some(ids)))) => {
                self.cursor = (ids.len() == 32).then(|| ids.last().expect("full page").clone());
                self.queued = ids
                    .into_iter()
                    .filter(|id| !self.active.contains(id))
                    .collect();
            }
            Ok((
                _,
                Ok(None)
                | Err(
                    ChannelWorkError::Changed
                    | ChannelWorkError::Suspended
                    | ChannelWorkError::Unsupported,
                ),
            )) => {}
            _ => {
                let now = tokio::time::Instant::now();
                if self
                    .last_warning
                    .is_none_or(|prior| now - prior >= Duration::from_secs(30))
                {
                    tracing::warn!(
                        target: "cma::bridge::operations",
                        operation = "scheduler_failure", worker = self.port.kind().label(),
                        "Channel work could not advance; durable identity retained for retry"
                    );
                    self.last_warning = Some(now);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "channel_scheduler_tests.rs"]
mod tests;
