//! Process-local access revocation, refreshed independently of request traffic.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use axum::{http::header, response::Response};
use futures_util::{future::BoxFuture, future::Shared, FutureExt};
use layer2_core::{
    account_deletion::{
        subject_hash, AccountDeletionError, AccountStanding, OwnerSessionAuthority,
    },
    resource_access::{CredentialSource, ResourceActor, ResourceOwner},
    sandbox::agent_io::CommandAuthorizationError,
    Caller, SubjectId,
};

use super::{AuthMode, AuthState, RequestAuth};
use crate::store::account_deletions::MongoAccountDeletionStore;

const MAX_SNAPSHOT_AGE: Duration = Duration::from_secs(10);
const REFRESH_PERIOD: Duration = Duration::from_secs(1);
const MAX_FALLBACK_SUBJECTS: usize = 256;
type PointLookup =
    Shared<BoxFuture<'static, (tokio::time::Instant, Result<bool, AccountDeletionError>)>>;

struct PointWaiter<'a> {
    fallback: &'a Mutex<BTreeMap<String, PointLookup>>,
    hash: &'a str,
    lookup: Option<PointLookup>,
}
impl PointWaiter<'_> {
    async fn wait(mut self) -> Result<bool, AccountDeletionError> {
        self.lookup.as_mut().expect("live standing lookup").await.1
    }
}
impl Drop for PointWaiter<'_> {
    fn drop(&mut self) {
        let Some(lookup) = self.lookup.take() else {
            return;
        };
        let mut fallback = self.fallback.lock().expect("standing fallback lock");
        // A normally completed waiter has no Shared identity left. The final
        // cancelled waiter removes its lookup while other frames keep theirs.
        if fallback
            .get(self.hash)
            .is_some_and(|current| current.ptr_eq(&lookup) && current.strong_count() == Some(2))
        {
            fallback.remove(self.hash);
        }
        // Serialize dropping the caller reference with other waiter drops, so
        // simultaneous cancellations cannot leave a map-only pending future.
        drop(lookup);
    }
}

#[async_trait::async_trait]
trait SnapshotSource: Send + Sync {
    async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError>;
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError>;
}

#[async_trait::async_trait]
impl SnapshotSource for MongoAccountDeletionStore {
    async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError> {
        MongoAccountDeletionStore::active_hashes(self).await
    }
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        MongoAccountDeletionStore::erasing(self, subject).await
    }
}

#[derive(Default)]
struct Snapshot {
    hashes: BTreeSet<String>,
    refreshed_at: Option<Instant>,
    generation: u64,
    local: BTreeMap<String, u64>,
}

pub(crate) struct ErasingSnapshot {
    source: Arc<dyn SnapshotSource>,
    state: RwLock<Snapshot>,
    refresh_lock: tokio::sync::Mutex<()>,
    fallback: Mutex<BTreeMap<String, PointLookup>>,
}

impl ErasingSnapshot {
    pub(crate) fn new(source: Arc<MongoAccountDeletionStore>) -> Self {
        Self::with_source(source)
    }
    fn with_source(source: Arc<dyn SnapshotSource>) -> Self {
        Self {
            source,
            state: RwLock::new(Snapshot::default()),
            refresh_lock: tokio::sync::Mutex::new(()),
            fallback: Mutex::new(BTreeMap::new()),
        }
    }
    pub(crate) async fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        if let Err(error) = self.refresh().await {
            tracing::warn!(%error, "initial account standing snapshot unavailable; using point reads");
        }
        self.spawn_refresh()
    }
    pub(crate) async fn refresh(&self) -> Result<(), AccountDeletionError> {
        let _refresh = self.refresh_lock.lock().await;
        let generation = self
            .state
            .read()
            .expect("standing snapshot lock")
            .generation;
        let mut hashes = self.source.active_hashes().await?;
        let mut state = self.state.write().expect("standing snapshot lock");
        // A query started before acceptance cannot erase the synchronous local
        // insertion made after its durable commit.
        state.local.retain(|_, inserted| *inserted > generation);
        hashes.extend(state.local.keys().cloned());
        state.hashes = hashes;
        state.refreshed_at = Some(Instant::now());
        Ok(())
    }
    pub(crate) fn spawn_refresh(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let snapshot = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval_at(
                tokio::time::Instant::now() + REFRESH_PERIOD,
                REFRESH_PERIOD,
            );
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticks.tick().await;
                let Some(snapshot) = snapshot.upgrade() else {
                    break;
                };
                if let Err(error) = snapshot.refresh().await {
                    tracing::warn!(%error, "account standing snapshot refresh failed");
                }
            }
        })
    }
    fn snapshot_result(&self, hash: &str) -> Option<bool> {
        let state = self.state.read().expect("standing snapshot lock");
        if state.local.contains_key(hash) {
            return Some(true);
        }
        state
            .refreshed_at
            .filter(|at| at.elapsed() <= MAX_SNAPSHOT_AGE)
            .map(|_| state.hashes.contains(hash))
    }
    fn point_lookup(
        &self,
        hash: &str,
        subject: &SubjectId,
    ) -> Result<PointLookup, AccountDeletionError> {
        let mut fallback = self.fallback.lock().expect("standing fallback lock");
        if let Some(lookup) = fallback.get(hash) {
            let reusable = match lookup.peek() {
                None => lookup.strong_count().is_some_and(|count| count > 1),
                // Completed denials and source failures remain conservative for
                // at most one refresh period. Allows are shared only in flight.
                Some((at, result)) => !matches!(result, Ok(false)) && at.elapsed() < REFRESH_PERIOD,
            };
            if reusable {
                return Ok(lookup.clone());
            }
            fallback.remove(hash);
        }
        if fallback.len() >= MAX_FALLBACK_SUBJECTS {
            let removable = fallback
                .iter()
                .find(|(_, lookup)| lookup.peek().is_some() || lookup.strong_count() == Some(1))
                .map(|(hash, _)| hash.clone());
            let Some(removable) = removable else {
                return Err(AccountDeletionError::Unavailable);
            };
            fallback.remove(&removable);
        }
        let source = self.source.clone();
        let subject = subject.clone();
        // Shared polling keeps other frames alive when one is cancelled. The
        // final waiter drops an unfinished read immediately; no detached task
        // retains the source or drains work after cancellation or shutdown.
        let lookup = async move {
            let result = source.erasing(&subject).await;
            (tokio::time::Instant::now(), result)
        }
        .boxed()
        .shared();
        fallback.insert(hash.to_owned(), lookup.clone());
        Ok(lookup)
    }
}

#[async_trait::async_trait]
impl AccountStanding for ErasingSnapshot {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        let hash = subject_hash(subject);
        if let Some(result) = self.snapshot_result(&hash) {
            return Ok(result);
        }
        let result = match self.point_lookup(&hash, subject) {
            Ok(lookup) => {
                PointWaiter {
                    fallback: &self.fallback,
                    hash: &hash,
                    lookup: Some(lookup),
                }
                .wait()
                .await
            }
            Err(error) => Err(error),
        };
        // A denial learned while the read was in flight overrides its allow.
        // A concurrent refresh cannot weaken an authority refusal or failure.
        if self.snapshot_result(&hash) == Some(true) {
            Ok(true)
        } else {
            result
        }
    }
    async fn authoritative(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        self.source.erasing(subject).await
    }
    fn insert_local(&self, hash: &str) {
        let mut state = self.state.write().expect("standing snapshot lock");
        state.generation = state
            .generation
            .checked_add(1)
            .expect("standing generation exhausted");
        let generation = state.generation;
        state.local.insert(hash.to_owned(), generation);
        state.hashes.insert(hash.to_owned());
    }
}

pub(super) fn allowed_while_erasing(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        (
            "GET",
            "/auth/session" | "/api/v1/account/deletion" | "/auth/logout"
        ) | ("POST", "/auth/logout")
    )
}

impl AuthState {
    pub(crate) fn with_account_standing(
        mut self: Arc<Self>,
        standing: Arc<dyn AccountStanding>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("standing configured before sharing auth")
            .standing = standing;
        self
    }
    pub(crate) fn with_account_deletion_enabled(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("deletion policy configured before sharing auth")
            .account_deletion_enabled = enabled;
        self
    }
    pub(super) fn owner_session_authority(
        &self,
        request: &RequestAuth,
    ) -> Option<OwnerSessionAuthority> {
        let personal = request.binding.is_none()
            && matches!(
                request.credential_source,
                CredentialSource::Retained | CredentialSource::Independent { .. }
            )
            && match request.delegated_credential.as_deref() {
                Some(token) => {
                    !super::agent_keys::is_agent_key(token)
                        && super::bearer::permits_personal_authority(token)
                }
                None => {
                    request.credential_source == CredentialSource::Retained
                        && matches!(self.mode, AuthMode::Trusted { .. })
                }
            };
        personal.then(|| OwnerSessionAuthority::verified_personal(request.subject.clone()))
    }
    pub(super) fn session_entitlements(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
    ) -> Vec<&'static str> {
        super::entitlements::reported(caller, authority, self.account_deletion_enabled)
    }
    pub(super) async fn account_gate(
        &self,
        method: &str,
        path: &str,
        subject: &SubjectId,
        owner_session: bool,
    ) -> Option<Response> {
        if path == "/api/v1/account/deletion" && matches!(method, "GET" | "POST") && !owner_session
        {
            return Some(problem(AccountDeletionError::RequiresOwnerSession));
        }
        if allowed_while_erasing(method, path)
            || (method == "POST" && path == "/api/v1/account/deletion" && owner_session)
        {
            return None;
        }
        match self.standing.erasing(subject).await {
            Ok(false) => None,
            Ok(true) => Some(deletion_problem()),
            Err(_) => Some(unavailable()),
        }
    }
    pub(super) async fn require_command_account(
        &self,
        subject: &SubjectId,
    ) -> Result<(), CommandAuthorizationError> {
        if self
            .standing
            .authoritative(subject)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(())
    }
    pub(super) async fn require_actor_owner_accounts(
        &self,
        actor: &ResourceActor,
        owner: &ResourceOwner,
    ) -> Result<(), CommandAuthorizationError> {
        // Each deferred authorization boundary performs at most one projected
        // actor read and one distinct personal-owner read, never per stream frame.
        if let Some(subject) = actor.ordinary_subject() {
            self.require_command_account(subject).await?;
        }
        if let ResourceOwner::Person { subject } = owner {
            if actor.ordinary_subject() != Some(subject) {
                self.require_command_account(subject).await?;
            }
        }
        Ok(())
    }
}

pub(super) fn deletion_problem() -> Response {
    problem(AccountDeletionError::InProgress)
}
fn problem(error: AccountDeletionError) -> Response {
    let mut response = layer1_public::rest::account_deletion_problem(error);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static cache policy"),
    );
    response
}
pub(super) fn unavailable() -> Response {
    let mut response = super::middleware::store_failure();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static cache policy"),
    );
    response
}

#[cfg(test)]
#[path = "account_standing_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "account_standing_fallback_tests.rs"]
mod fallback_tests;

#[cfg(test)]
#[path = "account_standing_mongo_tests.rs"]
mod mongo_tests;
