//! Conservative denial cache; every allow requires a fresh authority read.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use axum::{http::header, response::Response};
use layer2_core::{
    account_deletion::{
        subject_hash, AccountDeletionError, AccountStanding, OwnerSessionAuthority,
    },
    resource_access::{CredentialSource, ResourceActor, ResourceOwner},
    sandbox::agent_io::CommandAuthorizationError,
    Caller, SubjectId,
};

use super::{AuthMode, AuthState, RequestAuth};

const MAX_SNAPSHOT_AGE: Duration = Duration::from_secs(10);
const REFRESH_PERIOD: Duration = Duration::from_secs(1);
const MAX_POINT_READS: usize = 256;
const MAX_CACHED_DENIALS: usize = 256;

#[async_trait::async_trait]
pub trait SnapshotSource: Send + Sync {
    async fn active_hashes(&self) -> Result<BTreeSet<String>, AccountDeletionError>;
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError>;
}

#[derive(Default)]
struct Snapshot {
    hashes: BTreeSet<String>,
    refreshed_at: Option<Instant>,
    generation: u64,
    local: BTreeMap<String, u64>,
}

pub struct ErasingSnapshot {
    source: Arc<dyn SnapshotSource>,
    state: RwLock<Snapshot>,
    refresh_lock: tokio::sync::Mutex<()>,
    denials: Mutex<BTreeMap<String, tokio::time::Instant>>,
    point_reads: tokio::sync::Semaphore,
}

impl ErasingSnapshot {
    #[cfg(feature = "test-support")]
    pub fn set_fixture_snapshot_age(&self, age: Duration) {
        self.state.write().unwrap().refreshed_at = Some(Instant::now() - age);
    }

    #[cfg(feature = "test-support")]
    pub fn fixture_hashes_empty(&self) -> bool {
        self.state.read().unwrap().hashes.is_empty()
    }

    pub fn new(source: Arc<dyn SnapshotSource>) -> Self {
        Self::with_source(source)
    }
    fn with_source(source: Arc<dyn SnapshotSource>) -> Self {
        Self {
            source,
            state: RwLock::new(Snapshot::default()),
            refresh_lock: tokio::sync::Mutex::new(()),
            denials: Mutex::new(BTreeMap::new()),
            point_reads: tokio::sync::Semaphore::new(MAX_POINT_READS),
        }
    }
    pub async fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        if let Err(error) = self.refresh().await {
            tracing::warn!(%error, "initial account standing snapshot unavailable; using point reads");
        }
        self.spawn_refresh()
    }
    pub async fn refresh(&self) -> Result<(), AccountDeletionError> {
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
        self.denials.lock().expect("standing denial lock").clear();
        Ok(())
    }
    pub fn spawn_refresh(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
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
            .filter(|_| state.hashes.contains(hash))
            .map(|_| true)
    }
    fn cached_denial(&self, hash: &str) -> bool {
        self.snapshot_result(hash) == Some(true)
            || self
                .denials
                .lock()
                .expect("standing denial lock")
                .get(hash)
                .is_some_and(|at| at.elapsed() < REFRESH_PERIOD)
    }
    async fn point_read(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        let _permit = self
            .point_reads
            .try_acquire()
            .map_err(|_| AccountDeletionError::Unavailable)?;
        // An invocation cannot join an older read: that read may have observed
        // absence before another replica durably accepted deletion.
        self.source.erasing(subject).await
    }
    fn cache_denial(&self, hash: String) {
        let mut denials = self.denials.lock().expect("standing denial lock");
        denials.retain(|_, at| at.elapsed() < REFRESH_PERIOD);
        if denials.len() >= MAX_CACHED_DENIALS {
            denials.pop_first();
        }
        denials.insert(hash, tokio::time::Instant::now());
    }
}

#[async_trait::async_trait]
impl AccountStanding for ErasingSnapshot {
    async fn erasing(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        let hash = subject_hash(subject);
        if self.cached_denial(&hash) {
            return Ok(true);
        }
        let result = self.point_read(subject).await;
        if matches!(result, Ok(true)) {
            self.cache_denial(hash.clone());
        }
        // A denial learned during this read overrides its result.
        if self.cached_denial(&hash) {
            Ok(true)
        } else {
            result
        }
    }
    async fn authoritative(&self, subject: &SubjectId) -> Result<bool, AccountDeletionError> {
        self.point_read(subject).await
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
    pub fn with_account_standing(
        mut self: Arc<Self>,
        standing: Arc<dyn AccountStanding>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("standing configured before sharing auth")
            .standing = standing;
        self
    }
    pub fn with_account_deletion_enabled(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("deletion policy configured before sharing auth")
            .account_deletion_enabled = enabled;
        self
    }
    pub fn with_privacy_export_enabled(mut self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("export policy configured before sharing auth")
            .privacy_export_enabled = enabled;
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
    pub async fn require_command_account(
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
