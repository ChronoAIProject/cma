//! Shared verified observations. Subscriber permissions are never cached.
mod cache;
mod commands;
pub(in crate::sandbox) mod output;
mod worker;
pub use cache::{MAX_CACHED_BYTES, MAX_PAGE_BYTES};

use super::{
    AgentEventStreamAuthorization, AgentEventStreamHeader, AgentReadError, AgentReadService,
};
use crate::{resource_access::ResourceAccessSnapshot, AgentId};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::Instant,
};

pub const MAX_OBSERVED_AGENTS: usize = 256;
pub const MAX_OBSERVERS: usize = 1024;
pub const MAX_OBSERVATION_COHORT: usize = 100;
pub const OBSERVATION_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentObservationToken {
    pub agent_id: AgentId,
    pub created_at: String,
    pub instance_id: String,
    pub epoch: u64,
}

/// Read custody is separate from permission and from native effect ownership.
pub struct AgentObservationAuthority {
    pub token: AgentObservationToken,
    deadline: watch::Sender<Option<Instant>>,
}
impl AgentObservationAuthority {
    pub fn new(token: AgentObservationToken, until: Instant) -> Arc<Self> {
        Arc::new(Self {
            token,
            deadline: watch::channel(Some(until)).0,
        })
    }
    pub fn current(&self) -> bool {
        self.deadline
            .borrow()
            .is_some_and(|until| Instant::now() < until)
    }
    pub fn revoke(&self) {
        self.deadline.send_replace(None);
    }
    pub fn renew(&self, until: Instant) -> Result<(), AgentReadError> {
        let mut valid = false;
        self.deadline.send_modify(|deadline| {
            if deadline.is_some_and(|old| Instant::now() < old) && Instant::now() < until {
                *deadline = Some(until);
                valid = true;
            } else {
                *deadline = None;
            }
        });
        if valid {
            Ok(())
        } else {
            Err(unavailable())
        }
    }
}

pub struct AgentObservationRefresh {
    pub token: AgentObservationToken,
    pub until: Instant,
    /// None means another claimed refresh has not yet reached its next slot.
    pub snapshot: Option<Result<ResourceAccessSnapshot, AgentReadError>>,
}

#[async_trait::async_trait]
pub trait AgentObservationStore: Send + Sync {
    fn ready(&self) -> bool;
    async fn source_generation(
        &self,
        agent: &super::Agent,
    ) -> Result<Option<Vec<u8>>, super::store::SandboxStoreError>;
    /// Bind reusable output evidence to its independently published projection
    /// generation and current integrity state as well as the aggregate.
    async fn output_generation(
        &self,
        agent: &super::Agent,
    ) -> Result<Option<Vec<u8>>, super::store::SandboxStoreError> {
        self.source_generation(agent).await
    }
    async fn commands(
        &self,
        _ids: &[crate::ids::AgentResponseId],
    ) -> Result<Vec<Result<super::agent_io::AdmittedAgentCommand, AgentReadError>>, AgentReadError>
    {
        Err(unavailable())
    }
    /// Current read custody and resource facts for release requests only. No
    /// earlier observation is permission to release a later buffered result.
    async fn fresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<Result<ResourceAccessSnapshot, AgentReadError>>, AgentReadError>;
    /// Atomically claim periodic slots, renew exact read epochs, then read fresh
    /// facts for the claimed cohort. Successors retain the next-refresh bound.
    async fn refresh(
        &self,
        tokens: &[AgentObservationToken],
    ) -> Result<Vec<AgentObservationRefresh>, AgentReadError>;
}

tokio::task_local! {
    static READ_CUSTODY: Arc<AgentObservationAuthority>;
    static DURABLE_READ_FALLBACK: ();
}

/// Available only while opening a routed read. It grants no native effects.
pub fn observation_agent() -> Option<AgentId> {
    READ_CUSTODY
        .try_with(|authority| {
            authority
                .current()
                .then(|| authority.token.agent_id.clone())
        })
        .ok()
        .flatten()
}

/// A control may observe durable results without another
/// shared subscription when observer admission is full. This changes no access
/// checks or release fences and grants no read custody or native authority.
pub async fn durable_read_scope<F: std::future::Future>(future: F) -> F::Output {
    DURABLE_READ_FALLBACK.scope((), future).await
}

pub(super) fn durable_read_fallback() -> bool {
    DURABLE_READ_FALLBACK.try_with(|_| ()).is_ok()
}

pub async fn observation_scope<F: std::future::Future>(
    authority: Arc<AgentObservationAuthority>,
    future: F,
) -> F::Output {
    READ_CUSTODY.scope(authority, future).await
}

struct Lane {
    authority: Arc<AgentObservationAuthority>,
    flight: Arc<tokio::sync::Mutex<()>>,
    output_waiters: Arc<Mutex<output::GenerationWaiters>>,
    hint_task: Option<tokio::task::JoinHandle<()>>,
    subscribers: BTreeMap<u64, Subscriber>,
}
impl Drop for Lane {
    fn drop(&mut self) {
        if let Some(task) = &self.hint_task {
            task.abort();
        }
    }
}
struct Subscriber {
    authorization: AgentEventStreamAuthorization,
    terminal: bool,
    notice: watch::Sender<Option<Result<AgentEventStreamHeader, AgentReadError>>>,
}
struct Registry {
    lanes: BTreeMap<String, Lane>,
    next_id: u64,
    subscribers: usize,
}
struct Fence {
    authorization: AgentEventStreamAuthorization,
    reply: oneshot::Sender<Result<ResourceAccessSnapshot, AgentReadError>>,
}

pub struct AgentObservationService {
    reads: Arc<AgentReadService>,
    store: Arc<dyn AgentObservationStore>,
    registry: Mutex<Registry>,
    fences: mpsc::Sender<Fence>,
    commands: mpsc::Sender<commands::Request>,
    cache: Mutex<cache::Cache>,
    updates: std::sync::OnceLock<Arc<dyn super::AgentUpdateRecorder>>,
    hinted: Mutex<std::collections::BTreeSet<String>>,
    wake: tokio::sync::Notify,
}

impl AgentObservationService {
    pub fn new(reads: Arc<AgentReadService>, store: Arc<dyn AgentObservationStore>) -> Arc<Self> {
        let (fences, receive) = mpsc::channel(MAX_OBSERVERS);
        let (commands, command_receive) = mpsc::channel(MAX_OBSERVERS);
        let service = Arc::new(Self {
            reads,
            store,
            fences,
            commands,
            cache: Mutex::default(),
            updates: std::sync::OnceLock::new(),
            hinted: Mutex::default(),
            wake: tokio::sync::Notify::new(),
            registry: Mutex::new(Registry {
                lanes: BTreeMap::new(),
                next_id: 0,
                subscribers: 0,
            }),
        });
        tokio::spawn(worker::run(Arc::downgrade(&service), receive));
        tokio::spawn(commands::run(Arc::downgrade(&service), command_receive));
        service
    }

    pub fn install_hints(
        &self,
        updates: Arc<dyn super::AgentUpdateRecorder>,
    ) -> Result<(), AgentReadError> {
        self.updates.set(updates).map_err(|_| unavailable())
    }

    fn hint_task(self: &Arc<Self>, id: &AgentId) -> Option<tokio::task::JoinHandle<()>> {
        let mut heads = self.updates.get()?.subscribe_head(id)?;
        let service = Arc::downgrade(self);
        let id = id.to_string();
        Some(tokio::spawn(async move {
            while heads.recv().await.is_ok() {
                let Some(service) = service.upgrade() else {
                    return;
                };
                let registry = service.registry.lock().unwrap();
                if registry.lanes.contains_key(&id) {
                    service.hinted.lock().unwrap().insert(id.clone());
                    service.wake.notify_one();
                }
            }
        }))
    }

    pub fn subscribe(
        self: &Arc<Self>,
        authorization: AgentEventStreamAuthorization,
    ) -> Result<AgentObservationSubscription, AgentReadError> {
        let authority = READ_CUSTODY
            .try_with(Arc::clone)
            .map_err(|_| unavailable())?;
        if !self.store.ready()
            || !authority.current()
            || authority.token.agent_id != authorization.id
            || authorization.incarnation() != Some(authority.token.created_at.as_str())
        {
            return Err(unavailable());
        }
        let mut registry = self.registry.lock().unwrap();
        let agent = authorization.id.to_string();
        if registry
            .lanes
            .get(&agent)
            .is_some_and(|lane| !lane.authority.current())
        {
            if let Some(lane) = registry.lanes.remove(&agent) {
                lane.authority.revoke();
                registry.subscribers -= lane.subscribers.len();
            }
        }
        if registry.subscribers >= MAX_OBSERVERS
            || (!registry.lanes.contains_key(&agent) && registry.lanes.len() >= MAX_OBSERVED_AGENTS)
        {
            return Err(unavailable());
        }
        registry.next_id = registry.next_id.checked_add(1).ok_or_else(unavailable)?;
        let id = registry.next_id;
        let lane = registry.lanes.entry(agent).or_insert_with(|| Lane {
            authority: authority.clone(),
            flight: Arc::new(tokio::sync::Mutex::new(())),
            output_waiters: Arc::default(),
            hint_task: self.hint_task(&authorization.id),
            subscribers: BTreeMap::new(),
        });
        if lane.authority.token != authority.token || !lane.authority.current() {
            return Err(unavailable());
        }
        let authority = lane.authority.clone();
        let (notice, receiver) = watch::channel(None);
        lane.subscribers.insert(
            id,
            Subscriber {
                authorization: authorization.clone(),
                terminal: false,
                notice,
            },
        );
        registry.subscribers += 1;
        Ok(AgentObservationSubscription {
            service: Arc::downgrade(self),
            authority,
            authorization,
            id,
            receiver,
        })
    }

    pub async fn fresh(
        &self,
        authorization: &AgentEventStreamAuthorization,
    ) -> Result<ResourceAccessSnapshot, AgentReadError> {
        if !self.store.ready() {
            return Err(unavailable());
        }
        let (reply, receive) = oneshot::channel();
        self.fences
            .try_send(Fence {
                authorization: authorization.clone(),
                reply,
            })
            .map_err(|_| unavailable())?;
        authorization
            .bounded(async { receive.await.map_err(|_| unavailable())? })
            .await
    }
}

/// One coalesced notification slot. Durable payload is fetched through a cursor;
/// a slow reader never builds an unbounded event backlog in the observer.
pub struct AgentObservationSubscription {
    service: Weak<AgentObservationService>,
    authority: Arc<AgentObservationAuthority>,
    authorization: AgentEventStreamAuthorization,
    id: u64,
    receiver: watch::Receiver<Option<Result<AgentEventStreamHeader, AgentReadError>>>,
}
impl AgentObservationSubscription {
    pub(in crate::sandbox) fn id(&self) -> u64 {
        self.id
    }
    pub async fn changed(&mut self) -> Result<AgentEventStreamHeader, AgentReadError> {
        self.authorization
            .clone()
            .bounded(async {
                self.receiver.changed().await.map_err(|_| unavailable())?;
                if !self.current() {
                    return Err(unavailable());
                }
                self.receiver
                    .borrow_and_update()
                    .clone()
                    .ok_or_else(unavailable)?
            })
            .await
    }
    pub fn current(&self) -> bool {
        !self.receiver.borrow().as_ref().is_some_and(Result::is_err) && !self.custody_lost()
    }

    /// Reports observation infrastructure loss independently of the caller's
    /// policy decision and of subscriber admission limits.
    pub fn custody_lost(&self) -> bool {
        !self.authority.current()
            || self
                .service
                .upgrade()
                .is_none_or(|service| !service.store.ready())
    }
}
impl Drop for AgentObservationSubscription {
    fn drop(&mut self) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        let mut registry = service.registry.lock().unwrap();
        let agent = self.authorization.id.to_string();
        if let Some(lane) = registry.lanes.get_mut(&agent) {
            if lane.subscribers.remove(&self.id).is_some() {
                registry.subscribers -= 1;
            }
        }
        if registry
            .lanes
            .get(&agent)
            .is_some_and(|lane| lane.subscribers.is_empty())
        {
            registry.lanes.remove(&agent);
            service.hinted.lock().unwrap().remove(&agent);
        }
    }
}

fn unavailable() -> AgentReadError {
    AgentReadError::Unavailable(
        "shared observation unavailable; reconnect from acknowledged progress".into(),
    )
}
