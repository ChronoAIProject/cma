//! Production record-backed source ownership, Identify quota and resume journal.
use super::{
    discovery::{DiscordApi, Discovery},
    journal_model::*,
    record_store::Store,
    status::{FailureCode, Stage, StatusAttempt},
    Checkpoint, Dispatch, GatewayError, NativeDiscordJournal,
};
use crate::{
    connections::store::index_id,
    records::{RecordClient, RecordError},
    turns::observer::ActivityObserver,
};
use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{open_blob, MasterKeyRing},
};
use futures_util::future::BoxFuture;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

pub struct DurableJournal {
    pub(super) store: Store,
    pub(super) authority: Authority,
    pub(super) id: String,
    pub(super) activity: Arc<dyn ActivityObserver>,
    pub(super) deadline: RwLock<tokio::time::Instant>,
    worker: String,
    epoch: u64,
    pub(super) api: DiscordApi,
    serial: Mutex<()>,
    pub(super) status: StatusAttempt,
}
impl DurableJournal {
    /// Read the actual sealed source and verify its bot before acquiring a lease.
    /// This never sends a Discord message or opens a Gateway socket.
    pub async fn acquire(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        record: &str,
        api: DiscordApi,
        activity: Arc<dyn ActivityObserver>,
    ) -> Result<Arc<Self>, GatewayError> {
        Self::acquire_store(Store::new(records, keys), record, api, activity).await
    }
    pub(crate) async fn acquire_observed(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        record: &str,
        api: DiscordApi,
        activity: Arc<dyn ActivityObserver>,
        status: StatusAttempt,
    ) -> Result<Arc<Self>, GatewayError> {
        Self::acquire_store_observed(Store::new(records, keys), record, api, activity, status).await
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn acquire_replicated(
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        record: &str,
        api: DiscordApi,
        activity: Arc<dyn ActivityObserver>,
        status: StatusAttempt,
        membership: &crate::replicas::membership::Membership,
    ) -> Result<Arc<Self>, GatewayError> {
        let replica = crate::replicas::store::Store::new(records.clone(), keys.clone())
            .map_err(super::source_fence::failure)?;
        let mut store = Store::new(records, keys);
        let clock = membership.clone();
        store.clock = Arc::new(move || clock.now_millis());
        store.replica = Some(replica);
        Self::acquire_store_observed(store, record, api, activity, status).await
    }
    pub(super) async fn acquire_store(
        store: Store,
        record: &str,
        api: DiscordApi,
        activity: Arc<dyn ActivityObserver>,
    ) -> Result<Arc<Self>, GatewayError> {
        Self::acquire_store_observed(store, record, api, activity, StatusAttempt::default()).await
    }
    pub(super) async fn acquire_store_observed(
        store: Store,
        record: &str,
        api: DiscordApi,
        activity: Arc<dyn ActivityObserver>,
        status: StatusAttempt,
    ) -> Result<Arc<Self>, GatewayError> {
        status.step(Stage::Resolve);
        let api = api.with_status(status.clone());
        let authority = status.result(load_authority(&store, record).await)?;
        let observed = api.discover(&authority.token, &authority.bot_id).await?;
        status.step(Stage::Lease);
        let id = index_id("discord-gateway-ownership-v1", &[&authority.bot_id]);
        let (now, started) = store.sample().await?;
        let loaded = status.result(store.load::<Lease>(&id, false).await)?;
        let expected = loaded.as_ref().map(|(_, version)| *version);
        let expired = loaded
            .as_ref()
            .map(|(lease, _)| crate::records::mongo::LeaseCondition::Expired(lease.lease_until));
        let worker = uuid::Uuid::new_v4().to_string();
        let mut lease = match loaded {
            Some((mut lease, _)) => {
                let refusal =
                    if lease.kind != "discord_gateway_v1" || lease.bot_id != authority.bot_id {
                        Some(FailureCode::LeaseInvalid)
                    } else if lease.owner != authority.owner {
                        Some(FailureCode::LeaseOwnerMismatch)
                    } else if lease.channel_record != record {
                        Some(FailureCode::LeaseSourceMismatch)
                    } else if lease.lease_until > now {
                        Some(FailureCode::LeaseContended)
                    } else {
                        None
                    };
                if let Some(reason) = refusal {
                    status.lease_refused(reason);
                    return Err(GatewayError::Authority);
                }
                if lease.revision != authority.revision {
                    lease.checkpoint = None;
                }
                lease.epoch = lease.epoch.checked_add(1).ok_or(GatewayError::Protocol)?;
                lease.revision = authority.revision.clone();
                lease.worker = worker.clone();
                lease.authorization = None;
                lease
            }
            None => Lease {
                kind: "discord_gateway_v1".into(),
                bot_id: authority.bot_id.clone(),
                owner: authority.owner.clone(),
                channel_record: record.into(),
                revision: authority.revision.clone(),
                worker: worker.clone(),
                epoch: 1,
                lease_until: 0,
                remaining: observed.session_start_limit.remaining,
                reset_at: observed.session_start_limit.reset_deadline(now),
                identify_after: 0,
                authorization: None,
                checkpoint: None,
            },
        };
        lease.lease_until = now + store.lease_ms();
        let epoch = lease.epoch;
        if let Err(error) = store.put_lease(&id, &lease, expected, expired).await {
            // A lost create/CAS response may already have acquired our exact lease.
            let retained = store.load::<Lease>(&id, false).await?;
            if !retained.is_some_and(|(r, _)| {
                r.worker == worker
                    && r.epoch == epoch
                    && r.revision == authority.revision
                    && r.lease_until > (store.clock)()
            }) {
                return Err(error);
            }
        }
        status.lease_acquired(lease_remaining(lease.lease_until, (store.clock)()));
        let deadline = started + std::time::Duration::from_millis(store.lease_ms() as u64);
        if store.replica.is_some() && tokio::time::Instant::now() >= deadline {
            return Err(GatewayError::Authority);
        }
        Ok(Arc::new(Self {
            deadline: RwLock::new(deadline),
            store,
            authority,
            id,
            activity,
            worker,
            epoch,
            api,
            serial: Mutex::new(()),
            status,
        }))
    }
    pub async fn checkpoint(&self) -> Result<Option<Checkpoint>, GatewayError> {
        Ok(self.fenced().await?.0.checkpoint)
    }
    /// Supervision must renew at least every ten seconds, and stop its socket on
    /// any failure. Fenced admission also checks the live channel without cache.
    pub async fn renew(&self) -> Result<(), GatewayError> {
        let _guard = self.serial.lock().await;
        self.current_authority().await?;
        let (mut lease, version) = self.fenced().await?;
        lease.lease_until = self.store.sample().await?.0 + self.store.lease_ms();
        let result = self.save(&lease, version).await;
        if result.is_ok() {
            self.status
                .lease_renewed(lease_remaining(lease.lease_until, (self.store.clock)()));
        }
        result
    }
    pub(super) async fn discard_checkpoint(&self, release_lease: bool) -> Result<(), GatewayError> {
        let _guard = self.serial.lock().await;
        let (mut lease, version) = self.fenced().await?;
        lease.checkpoint = None;
        if release_lease {
            lease.lease_until = 0;
        }
        self.save(&lease, version).await
    }
    pub async fn release(&self) -> Result<(), GatewayError> {
        let _guard = self.serial.lock().await;
        let (mut lease, version) = self.fenced().await?;
        lease.lease_until = 0;
        self.save(&lease, version).await
    }
    pub(super) async fn current_authority(&self) -> Result<Authority, GatewayError> {
        let current = load_authority(&self.store, &self.authority.record).await?;
        if current.revision != self.authority.revision || current.owner != self.authority.owner {
            return Err(GatewayError::Authority);
        }
        Ok(current)
    }
    pub(super) async fn fenced(&self) -> Result<(Lease, u64), GatewayError> {
        if !self.locally_live() {
            return Err(GatewayError::Authority);
        }
        let (lease, version) = self
            .store
            .load::<Lease>(&self.id, false)
            .await?
            .ok_or(GatewayError::Authority)?;
        if !self.locally_live()
            || lease.kind != "discord_gateway_v1"
            || lease.worker != self.worker
            || lease.epoch != self.epoch
            || lease.bot_id != self.authority.bot_id
            || lease.channel_record != self.authority.record
            || lease.revision != self.authority.revision
            || lease.owner != self.authority.owner
            || lease.lease_until <= (self.store.clock)()
        {
            return Err(GatewayError::Authority);
        }
        Ok((lease, version))
    }
    async fn authorize(&self, resume: Option<&Checkpoint>) -> Result<(), GatewayError> {
        self.status.step(Stage::Authorize);
        let _guard = self.serial.lock().await;
        self.current_authority().await?;
        let observed = self
            .api
            .discover(&self.authority.token, &self.authority.bot_id)
            .await?;
        self.status.step(Stage::Authorize);
        let (mut lease, version) = self.fenced().await?;
        let now = (self.store.clock)();
        if lease.authorization.is_some() {
            return Err(GatewayError::Authority);
        }
        if let Some(checkpoint) = resume {
            if lease.checkpoint.as_ref() != Some(checkpoint) {
                return Err(GatewayError::Authority);
            }
        } else {
            if let Err(error) = reserve(&mut lease, &observed, now) {
                self.status.failure(if lease.remaining == 0 {
                    FailureCode::IdentifyBudgetExhausted
                } else {
                    FailureCode::IdentifyBucketWait
                });
                // Retain a newly observed reset boundary even when no Identify
                // can be reserved yet (including zero -> positive recovery).
                self.save(&lease, version).await?;
                return Err(error);
            }
        }
        lease.authorization = Some(self.worker.clone());
        lease.lease_until = now + self.store.lease_ms();
        self.save(&lease, version).await?;
        self.status.identify_reserved(resume.is_some());
        Ok(())
    }
    async fn commit(&self, checkpoint: &Checkpoint, event: &Dispatch) -> Result<(), GatewayError> {
        let _guard = self.serial.lock().await;
        let current = self.current_authority().await?;
        let (mut lease, version) = self.fenced().await?;
        if lease.authorization.as_deref() != Some(&self.worker)
            || checkpoint.sequence != event.sequence
        {
            return Err(GatewayError::Authority);
        }
        if let Some(previous) = &lease.checkpoint {
            if previous.session_id == checkpoint.session_id
                && checkpoint.sequence < previous.sequence
            {
                return Err(GatewayError::Protocol);
            }
            if previous.session_id != checkpoint.session_id && event.name != "READY" {
                return Err(GatewayError::Protocol);
            }
        } else if event.name != "READY" {
            return Err(GatewayError::Protocol);
        }
        // The transport validates READY; defend the concrete journal boundary too.
        let validated =
            super::protocol::checkpoint(lease.checkpoint.as_ref(), event, &current.bot_id)?;
        if validated != *checkpoint {
            return Err(GatewayError::Protocol);
        }
        if event.name == "MESSAGE_CREATE" {
            self.persist_message(&current, event).await?;
        }
        if event.name == "INTERACTION_CREATE" {
            self.persist_component(&current, event).await?;
        }
        if self.api.message_events
            && matches!(
                event.name.as_str(),
                "MESSAGE_UPDATE" | "MESSAGE_DELETE" | "MESSAGE_DELETE_BULK"
            )
        {
            self.persist_message_event(&current, event).await?;
        }
        if self.api.reactions && super::reactions::is_event(&event.name) {
            self.persist_reaction(&current, checkpoint, event).await?;
        }
        // Persisting an inbox record cannot advance the stream or launch work.
        // The checkpoint follows confirmed durable storage, under the same fence.
        self.current_authority().await?;
        if lease.lease_until <= (self.store.clock)() {
            return Err(GatewayError::Authority);
        }
        lease.checkpoint = Some(checkpoint.clone());
        lease.lease_until = self.store.sample().await?.0 + self.store.lease_ms();
        self.save(&lease, version).await
    }
}
impl NativeDiscordJournal for DurableJournal {
    fn connection_deadline(&self) -> Option<tokio::time::Instant> {
        self.store
            .replica
            .as_ref()
            .map(|_| *self.deadline.read().unwrap())
    }
    fn authorize_connection<'a>(
        &'a self,
        resume: Option<&'a Checkpoint>,
    ) -> BoxFuture<'a, Result<(), GatewayError>> {
        Box::pin(async move { self.status.result(self.authorize(resume).await) })
    }
    fn admit<'a>(
        &'a self,
        checkpoint: &'a Checkpoint,
        event: &'a Dispatch,
    ) -> BoxFuture<'a, Result<(), GatewayError>> {
        Box::pin(self.commit(checkpoint, event))
    }
}
fn reserve(lease: &mut Lease, observed: &Discovery, now: i64) -> Result<(), GatewayError> {
    let limit = &observed.session_start_limit;
    if limit.reset_after != 0 && now >= lease.reset_at {
        lease.remaining = limit.remaining;
        lease.reset_at = limit.reset_deadline(now);
    } else {
        // A newer Discord observation can reduce a local reservation budget,
        // never restore an Identify already reserved but not yet observed there.
        lease.remaining = lease.remaining.min(limit.remaining);
    }
    // A zero-duration observation supplies no future reset boundary. Never
    // replenish reservations on repeated zero/stale snapshots. When a positive
    // window becomes known, start tracking it without refunding reservations.
    if limit.reset_after == 0 || lease.reset_at == i64::MAX {
        lease.reset_at = limit.reset_deadline(now);
    }
    if now < lease.identify_after || lease.remaining == 0 {
        return Err(GatewayError::RateLimited);
    }
    lease.remaining -= 1;
    // This implementation is one shard (bucket zero), regardless of the bot's
    // advertised max_concurrency. The same bucket waits at least five seconds.
    lease.identify_after = now + 5_000;
    Ok(())
}
async fn load_authority(store: &Store, record: &str) -> Result<Authority, GatewayError> {
    load_authority_version(store, record)
        .await
        .map(|(authority, _)| authority)
}
pub(super) async fn load_authority_version(
    store: &Store,
    record: &str,
) -> Result<(Authority, u64), GatewayError> {
    let url = store
        .channels
        .record_url(record)
        .map_err(|_| GatewayError::InvalidConfiguration)?;
    let row = store
        .channels
        .get(url.as_str())
        .await
        .map_err(|error| match error {
            RecordError::NotFound => GatewayError::Authority,
            _ => GatewayError::Unavailable,
        })?;
    let bytes = open_blob(&store.keys, &row.blob)
        .map_err(|_| GatewayError::Authority)?
        .payload;
    let definition: ChannelDefinition =
        serde_json::from_slice(&bytes).map_err(|_| GatewayError::Authority)?;
    if definition.record_id.as_str() != record {
        return Err(GatewayError::Authority);
    }
    Ok((Authority::from_definition(&definition)?, row.version))
}

fn lease_remaining(until: i64, now: i64) -> std::time::Duration {
    std::time::Duration::from_millis(until.saturating_sub(now).clamp(0, LEASE_MS) as u64)
}
