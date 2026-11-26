use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use layer2_core::{trigger_domain::BrokerHandoffState, SubjectId};
use layer3_adapter::ingress_budget::{AdmittedError, IngressOwnershipPolicy, SharedOwned};
use tokio::sync::{oneshot, Semaphore};

use crate::auth::persistence::SecretValue;

use super::{
    native_codex::{DeviceLogin, NativeCodex, LOGIN_TIMEOUT},
    subscription_store::{
        PendingLogin, SubscriptionOperation, SubscriptionRecord, SubscriptionStore,
    },
    subscription_tokens::SubscriptionTokens,
    SubscriptionError,
};

#[path = "subscriptions_refresh.rs"]
pub(super) mod refresh;

const HEARTBEAT: Duration = Duration::from_secs(15);
const STALE_HEARTBEAT: Duration = Duration::from_secs(60);
const REFRESH_LEASE: Duration = Duration::from_secs(30);
const REFRESH_BUDGET: Duration = Duration::from_secs(8);
const CAS_ATTEMPTS: usize = 16;

pub(super) struct NativeLogin {
    device: DeviceLogin,
    completion: oneshot::Receiver<Result<SecretValue, SubscriptionError>>,
    // Dropping the sender also cancels an attempt which failed to become durable.
    cancel: Option<oneshot::Sender<()>>,
}

#[async_trait::async_trait]
pub(super) trait NativeSubscriptionAuth: Send + Sync {
    /// The producer admits process, pipe, cache and cleanup storage before use.
    /// The returned cache must retain this policy through its final physical free.
    fn refresh_admitted<'a>(
        &'a self,
        _cache: &'a SharedOwned<SecretValue>,
        _policy: &IngressOwnershipPolicy,
    ) -> Result<SubscriptionOperation<'a, SharedOwned<SecretValue>>, AdmittedError> {
        Err(AdmittedError::Unsupported(
            "admitted native subscription refresh",
        ))
    }

    async fn start(&self) -> Result<NativeLogin, SubscriptionError>;
    async fn refresh(&self, cache: &SecretValue) -> Result<SecretValue, SubscriptionError>;
}

struct PinnedNativeAuth {
    binary: PathBuf,
}

#[async_trait::async_trait]
impl NativeSubscriptionAuth for PinnedNativeAuth {
    async fn start(&self) -> Result<NativeLogin, SubscriptionError> {
        let mut native = NativeCodex::open(&self.binary, None).await?;
        let device = native.login().await?;
        let (done, completion) = oneshot::channel();
        let (cancel, cancelled) = oneshot::channel();
        let awaited = device.clone();
        tokio::spawn(async move {
            let outcome = tokio::select! {
                outcome = native.wait_login(&awaited) => outcome,
                _ = cancelled => {
                    let _ = native.cancel(&awaited.login_id).await;
                    Err(SubscriptionError::Cancelled)
                }
            };
            let _ = done.send(outcome);
        });
        Ok(NativeLogin {
            device,
            completion,
            cancel: Some(cancel),
        })
    }

    async fn refresh(&self, cache: &SecretValue) -> Result<SecretValue, SubscriptionError> {
        let mut native = NativeCodex::open(&self.binary, Some(cache)).await?;
        native.refresh().await
    }
}

pub(crate) struct CodexSubscriptions {
    store: Arc<dyn SubscriptionStore>,
    native: Arc<dyn NativeSubscriptionAuth>,
    worker: String,
    login_capacity: Arc<Semaphore>,
}

impl CodexSubscriptions {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "admitted source qualification pending")
    )]
    pub(crate) fn tokens_admitted<'a>(
        &'a self,
        owner: &'a SharedOwned<SubjectId>,
        force: bool,
        previous_account: Option<&'a SharedOwned<String>>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<SubscriptionOperation<'a, SharedOwned<SubscriptionTokens>>, AdmittedError> {
        refresh::admitted::tokens(self, owner, force, previous_account, policy)
    }

    pub(crate) fn new(store: Arc<dyn SubscriptionStore>) -> Arc<Self> {
        Arc::new(Self {
            store,
            native: Arc::new(PinnedNativeAuth {
                binary: PathBuf::from("/usr/local/bin/codex-native"),
            }),
            worker: uuid::Uuid::new_v4().to_string(),
            login_capacity: Arc::new(Semaphore::new(32)),
        })
    }

    pub(crate) async fn start(
        self: &Arc<Self>,
        owner: &SubjectId,
        intent: &str,
    ) -> Result<PendingLogin, SubscriptionError> {
        if intent.is_empty() || intent.len() > 256 || intent.chars().any(char::is_control) {
            return Err(SubscriptionError::Changed);
        }
        let initial = self.get_or_create(owner).await?;
        if let Some(pending) = initial
            .pending
            .as_ref()
            .filter(|pending| pending.intent == intent)
        {
            return Ok(pending.clone());
        }
        let permit = self
            .login_capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| SubscriptionError::Unavailable)?;
        let native = self.native.start().await?;
        let now = SystemTime::now();
        let pending = PendingLogin {
            intent: intent.to_owned(),
            device: native.device.clone(),
            expires_at: now + LOGIN_TIMEOUT,
            heartbeat_at: now,
            worker: self.worker.clone(),
            state: BrokerHandoffState::Pending,
        };
        let mut current = initial;
        let mut saved = false;
        for _ in 0..CAS_ATTEMPTS {
            if let Some(winner) = current
                .pending
                .as_ref()
                .filter(|pending| pending.intent == intent)
            {
                return Ok(winner.clone());
            }
            let mut next = current.clone();
            next.revision += 1;
            next.pending = Some(pending.clone());
            if self.store.replace(&current, next).await? {
                saved = true;
                break;
            }
            current = self.record(owner).await?;
        }
        if !saved {
            return Err(SubscriptionError::Changed);
        }
        let manager = self.clone();
        let owner = owner.clone();
        let enrolled = pending.clone();
        tokio::spawn(async move {
            let _permit = permit;
            manager.await_login(owner, enrolled, native).await;
        });
        Ok(pending)
    }

    pub(crate) async fn status(
        &self,
        owner: &SubjectId,
        intent: &str,
    ) -> Result<PendingLogin, SubscriptionError> {
        let record = self.record(owner).await?;
        let mut login = record
            .pending
            .filter(|login| login.intent == intent)
            .ok_or(SubscriptionError::Changed)?;
        if login.state == BrokerHandoffState::Pending {
            let now = SystemTime::now();
            if now >= login.expires_at {
                login.state = BrokerHandoffState::Expired;
            } else if login.heartbeat_at + STALE_HEARTBEAT < now {
                login.state = BrokerHandoffState::Failed;
            }
        }
        Ok(login)
    }

    pub(crate) async fn cancel(
        &self,
        owner: &SubjectId,
        intent: &str,
    ) -> Result<(), SubscriptionError> {
        self.update_login(owner, intent, None, Some(BrokerHandoffState::Cancelled))
            .await
    }

    async fn await_login(&self, owner: SubjectId, pending: PendingLogin, mut native: NativeLogin) {
        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        heartbeat.tick().await;
        let outcome = loop {
            tokio::select! {
                completed = &mut native.completion => break completed.unwrap_or(Err(SubscriptionError::Unavailable)),
                _ = heartbeat.tick() => {
                    if self.update_login(&owner, &pending.intent, None, None).await.is_err() {
                        if let Some(cancel) = native.cancel.take() { let _ = cancel.send(()); }
                        return;
                    }
                }
            }
        };
        let (cache, state) = match outcome {
            Ok(cache)
                if SubscriptionTokens::from_native_cache(&cache)
                    .is_ok_and(|tokens| tokens.usable_at(SystemTime::now())) =>
            {
                (Some(cache), BrokerHandoffState::Satisfied)
            }
            Err(SubscriptionError::Expired) => (None, BrokerHandoffState::Expired),
            Err(SubscriptionError::Cancelled) => (None, BrokerHandoffState::Cancelled),
            _ => (None, BrokerHandoffState::Failed),
        };
        if self
            .update_login(&owner, &pending.intent, cache, Some(state))
            .await
            .is_err()
        {
            tracing::warn!("Codex sign-in completion could not be published; retry sign-in if the connection expires");
        }
    }

    async fn update_login(
        &self,
        owner: &SubjectId,
        intent: &str,
        cache: Option<SecretValue>,
        state: Option<BrokerHandoffState>,
    ) -> Result<(), SubscriptionError> {
        for _ in 0..CAS_ATTEMPTS {
            let current = self.record(owner).await?;
            let mut next = current.clone();
            let login = next
                .pending
                .as_mut()
                .filter(|login| {
                    login.intent == intent && login.state == BrokerHandoffState::Pending
                })
                .ok_or(SubscriptionError::Changed)?;
            if SystemTime::now() >= login.expires_at && state != Some(BrokerHandoffState::Expired) {
                return Err(SubscriptionError::Expired);
            }
            login.heartbeat_at = SystemTime::now();
            if let Some(state) = state {
                login.state = state;
            }
            if let Some(cache) = cache.as_ref() {
                next.cache = Some(cache.clone());
                next.refresh_lease = None;
            }
            next.revision += 1;
            if self.store.replace(&current, next).await? {
                return Ok(());
            }
        }
        Err(SubscriptionError::Changed)
    }

    pub(crate) async fn tokens(
        &self,
        owner: &SubjectId,
        force: bool,
        previous_account: Option<&str>,
    ) -> Result<SubscriptionTokens, SubscriptionError> {
        tokio::time::timeout(
            REFRESH_BUDGET,
            self.tokens_within(owner, force, previous_account),
        )
        .await
        .map_err(|_| SubscriptionError::Unavailable)?
    }

    async fn tokens_within(
        &self,
        owner: &SubjectId,
        force: bool,
        previous_account: Option<&str>,
    ) -> Result<SubscriptionTokens, SubscriptionError> {
        refresh::tokens_within(&refresh::Legacy(self), owner, force, previous_account).await
    }

    async fn get_or_create(
        &self,
        owner: &SubjectId,
    ) -> Result<SubscriptionRecord, SubscriptionError> {
        if let Some(record) = self.store.get(owner).await? {
            return Ok(record);
        }
        let empty = SubscriptionRecord::empty(owner.clone());
        if self.store.insert(empty.clone()).await? {
            Ok(empty)
        } else {
            self.record(owner).await
        }
    }

    async fn record(&self, owner: &SubjectId) -> Result<SubscriptionRecord, SubscriptionError> {
        self.store
            .get(owner)
            .await?
            .filter(|record| record.owner == *owner)
            .ok_or(SubscriptionError::SignInRequired)
    }
}

#[cfg(test)]
#[path = "subscriptions_tests.rs"]
mod tests;
