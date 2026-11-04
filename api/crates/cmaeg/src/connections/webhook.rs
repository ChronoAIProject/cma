//! Durable intent and bounded leases coordinate gateway-owned webhook mutations.
//!
//! Telegram has no conditional webhook mutation. These leases serialize gateway
//! callers and retain uncertain operations for reconciliation; they cannot fence
//! an external administrator or a Telegram operation delayed beyond its lease.
use super::{model::*, store::Store, Error};
use crate::{channels::management::ManagementState, telegram::TelegramClient};
use cmaeg_core::channel::Secret;
use std::time::Duration;

// Covers all three bounded eight-second HTTP calls, plus clock/scheduling margin.
const LEASE_SECONDS: i64 = 60;

pub(super) fn store(state: &ManagementState) -> Store {
    Store::new((*state.records).clone(), state.master_keys.clone())
}
pub(super) fn client(state: &ManagementState) -> Result<TelegramClient, Error> {
    match &state.telegram {
        Some(client) => Ok((**client).clone()),
        None => TelegramClient::production(Duration::from_secs(8)).map_err(|_| Error::Unavailable),
    }
}
pub(super) fn live(record: &Connection) -> bool {
    record
        .webhook_lease
        .as_ref()
        .is_some_and(|lease| lease.expires_at > now())
}
fn holds(record: &Connection, lease: &WebhookLease) -> bool {
    record
        .webhook_lease
        .as_ref()
        .is_some_and(|current| current.nonce == lease.nonce && current.kind == lease.kind)
}
pub(super) async fn claim(
    state: &ManagementState,
    mut record: Connection,
    version: u64,
    kind: WebhookOperation,
) -> Result<(Connection, WebhookLease), Error> {
    if live(&record) || (kind == WebhookOperation::Activate && record.webhook_retiring) {
        return Err(Error::Conflict);
    }
    let lease = WebhookLease {
        nonce: uuid::Uuid::new_v4().to_string(),
        expires_at: now() + LEASE_SECONDS,
        kind,
    };
    record.webhook_lease = Some(lease.clone());
    store(state).save_connection(&record, Some(version)).await?;
    Ok((record, lease))
}
pub(super) async fn may_activate(
    state: &ManagementState,
    id: &str,
    channel_id: &str,
    lease: &WebhookLease,
) -> Result<bool, Error> {
    let (current, _) = store(state).connection(id).await?;
    Ok(holds(&current, lease)
        && live(&current)
        && !current.webhook_retiring
        && current.status == Status::Ready
        && current.channel_id.as_deref() == Some(channel_id))
}
/// Release only a confirmed operation. A timeout remains durable until its lease
/// expires, so deletion/rebinding cannot overtake an uncertain platform mutation.
pub(super) async fn finish_activation(
    state: &ManagementState,
    id: &str,
    lease: &WebhookLease,
    confirmed: bool,
) -> Result<bool, Error> {
    for _ in 0..3 {
        let (mut current, version) = store(state).connection(id).await?;
        if !holds(&current, lease) {
            return Err(Error::Conflict);
        }
        let wanted = !current.webhook_retiring && current.status == Status::Ready;
        if !confirmed {
            return Ok(wanted);
        }
        current.webhook_lease = None;
        match store(state).save_connection(&current, Some(version)).await {
            Ok(_) => return Ok(wanted),
            Err(Error::Conflict) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(Error::Conflict)
}
pub(super) async fn retire(
    state: &ManagementState,
    mut record: Connection,
    version: u64,
) -> Result<(), Error> {
    if !record.webhook_retiring {
        record.webhook_retiring = true;
        store(state).save_connection(&record, Some(version)).await?;
    }
    Ok(())
}
/// Keep both pointers while an activation is live or cleanup is unconfirmed.
/// `release` is allowed only after the caller established the channel is absent.
pub(super) async fn cleanup(state: &ManagementState, id: &str, release: bool) -> Result<(), Error> {
    let (mut record, version) = store(state).connection(id).await?;
    if !record.webhook_retiring {
        return Err(Error::Conflict);
    }
    if live(&record) {
        return Err(Error::Conflict);
    }
    let Some(channel_id) = record.channel_id.clone() else {
        if release {
            record.channel_record = None;
            record.webhook_lease = None;
            record.webhook_retiring = false;
            store(state).save_connection(&record, Some(version)).await?;
        }
        return Ok(());
    };
    let (record, lease) = claim(state, record, version, WebhookOperation::Retire).await?;
    let material = record.material.as_ref().ok_or(Error::Unavailable)?;
    let token = Secret::new(material.token.clone());
    let telegram = client(state)?;
    let actual = telegram
        .get_webhook_url(&token)
        .await
        .map_err(|_| Error::Unavailable)?;
    // Check the lease immediately before the irreversible platform mutation.
    let (current, _) = store(state).connection(id).await?;
    if !holds(&current, &lease) || !live(&current) || !current.webhook_retiring {
        return Err(Error::Conflict);
    }
    if state
        .public_api_bases()
        .any(|base| actual == format!("{base}/v1/telegram/{channel_id}"))
    {
        telegram
            .delete_webhook(&token)
            .await
            .map_err(|_| Error::Unavailable)?;
    }
    for _ in 0..3 {
        let (mut current, version) = store(state).connection(id).await?;
        if !holds(&current, &lease) || !current.webhook_retiring {
            return Err(Error::Conflict);
        }
        current.webhook_lease = None;
        if release {
            current.channel_record = None;
            current.channel_id = None;
            current.webhook_retiring = false;
        }
        match store(state).save_connection(&current, Some(version)).await {
            Ok(_) => return Ok(()),
            Err(Error::Conflict) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(Error::Conflict)
}
