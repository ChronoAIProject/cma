//! Desktop authority is separate from Agent-manager ownership. A fixed durable
//! floor survives receiver boots and cannot be reset by changing runtime handles.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use layer3_adapter::desktop_ownership::wire::DesktopChallengeRequest;
use layer3_adapter::DesktopOwnership;
use tokio::{sync::watch, task::JoinSet, time::Instant};

use crate::failure::{Result, RuntimeError};

mod control;
mod persistence;
mod session;
pub(crate) use session::Session;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub sandbox: String,
    pub generation: u64,
    pub home: Option<String>,
    pub boot: String,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    receiver: Identity,
    owner: DesktopOwnership,
    retired: bool,
}

struct Challenge {
    request: DesktopChallengeRequest,
    deadline: Instant,
}

struct State {
    record: Option<Record>,
    poisoned: bool,
    legacy_closed: bool,
    deadline: Option<Instant>,
    revision: u64,
    reserved: bool,
    challenges: BTreeMap<String, Challenge>,
}

pub(crate) struct DesktopReceiver {
    pub(crate) controls: Arc<tokio::sync::Semaphore>,
    identity: Identity,
    root: PathBuf,
    state: Mutex<State>,
    // This mutex serializes admissions and draining. Join handles never leave
    // this set while a caller waits, so cancellation cannot detach old I/O.
    tasks: tokio::sync::Mutex<JoinSet<()>>,
    changed: watch::Sender<()>,
}

impl DesktopReceiver {
    pub(crate) fn new(root: PathBuf, identity: Identity) -> Arc<Self> {
        let loaded = persistence::load(&root);
        let poisoned = loaded.is_err();
        let record = loaded.ok().flatten();
        Arc::new(Self {
            controls: Arc::new(tokio::sync::Semaphore::new(16)),
            identity,
            root,
            state: Mutex::new(State {
                legacy_closed: poisoned || record.is_some(),
                record,
                poisoned,
                deadline: None,
                revision: 0,
                reserved: false,
                challenges: BTreeMap::new(),
            }),
            tasks: tokio::sync::Mutex::new(JoinSet::new()),
            changed: watch::channel(()).0,
        })
    }

    fn verify(&self, state: &mut State) -> Result<()> {
        if state.poisoned {
            return Err(unavailable());
        }
        // Detect missing/corrupt/changed custody on subsequent control calls.
        if persistence::load(&self.root).ok().as_ref() != Some(&state.record) {
            self.poison(state);
            return Err(unavailable());
        }
        Ok(())
    }

    fn poison(&self, state: &mut State) {
        state.poisoned = true;
        state.legacy_closed = true;
        state.deadline = None;
        state.challenges.clear();
        self.changed.send_replace(());
    }

    fn persist(&self, state: &mut State, record: Record) -> Result<()> {
        if persistence::save(&self.root, &record).is_err() {
            self.poison(state);
            return Err(unavailable());
        }
        state.record = Some(record);
        state.legacy_closed = true;
        Ok(())
    }

    fn check_owner(&self, owner: &DesktopOwnership) -> Result<()> {
        owner.validate().map_err(|_| refused())?;
        if owner.sandbox_id != self.identity.sandbox
            || owner
                .boot_id
                .as_ref()
                .is_some_and(|boot| boot != &self.identity.boot)
        {
            return Err(refused());
        }
        Ok(())
    }

    fn live(&self, state: &State, owner: &DesktopOwnership) -> bool {
        !state.poisoned
            && state.deadline.is_some_and(|until| until > Instant::now())
            && state.record.as_ref().is_some_and(|record| {
                !record.retired
                    && record.receiver == self.identity
                    && same_session(&record.owner, owner)
            })
    }

    pub(crate) async fn register<F, Fut>(
        self: &Arc<Self>,
        owner: Option<DesktopOwnership>,
        operation: F,
    ) -> Result<()>
    where
        F: FnOnce(Session) -> Fut,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().await;
        while tasks.try_join_next().is_some() {}
        let mut state = self.state.lock().unwrap();
        self.verify(&mut state)?;
        match &owner {
            Some(owner) => {
                self.check_owner(owner)?;
                if !self.live(&state, owner) || state.reserved {
                    return Err(refused());
                }
                state.reserved = true;
            }
            None if state.legacy_closed || tasks.len() >= 16 => return Err(refused()),
            None => {}
        }
        let session = Session {
            receiver: self.clone(),
            revision: state.revision,
            owner,
        };
        let finish = session.clone();
        let future = operation(session);
        tasks.spawn(async move {
            let _finish = session::Finish(finish);
            future.await;
        });
        Ok(())
    }
}

fn same_session(left: &DesktopOwnership, right: &DesktopOwnership) -> bool {
    left.sandbox_id == right.sandbox_id
        && left.runtime_id == right.runtime_id
        && left.boot_id == right.boot_id
        && left.session_id == right.session_id
        && left.epoch == right.epoch
}

fn refused() -> RuntimeError {
    RuntimeError::rejected(
        "desktop_ownership_stale",
        "desktop authority is absent, stale or expired",
    )
}

fn unavailable() -> RuntimeError {
    RuntimeError::unavailable(
        "desktop_ownership_unavailable",
        "desktop custody cannot be established",
    )
}

#[cfg(test)]
mod tests;
