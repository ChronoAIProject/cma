//! Repeat receipts share the lifecycle event's atomic commit and retention.
use layer2_core::{
    sandbox::{
        RetainedReopenHistory, RetainedReopenReceipt, RetainedReopenReplay,
        RetainedSuspendedReopenReceipt, Sandbox, SandboxEvent, SandboxEventKind, SandboxStatus,
    },
    SandboxId,
};
use mongodb::{
    bson::{doc, Document},
    options::{FindOneOptions, Hint, IndexOptions, ReadConcern},
    ClientSession, IndexModel,
};
use serde::{Deserialize, Serialize};

use super::{suspended, transaction, ApplyError, MongoSandboxStore, SandboxStoreError};
use crate::store::{
    sandbox_events::{event_id, read_event},
    sandbox_mongo::record_bson,
    sandboxes::read_sandbox,
};

const RECEIPT: &str = "retained_reopen_receipt";
const KEY: &str = "retained_reopen_receipt.key_sha256";
const REVISION: &str = "retained_reopen_receipt.accepted.revision";
const KEY_INDEX: &str = "sandbox_reopen_key_v1";
const REVISION_INDEX: &str = "sandbox_reopen_revision_v1";
const EVENT_SEQ: &str = "lifecycle_event_seq";

pub(in crate::store) fn indexes() -> [IndexModel; 2] {
    [
        (KEY_INDEX, doc! { "sandbox_id": 1, KEY: 1 }),
        (REVISION_INDEX, doc! { "sandbox_id": 1, REVISION: -1 }),
    ]
    .map(|(name, keys)| {
        IndexModel::builder()
            .keys(keys)
            .options(
                IndexOptions::builder()
                    .name(name.to_owned())
                    .unique(true)
                    .partial_filter_expression(doc! { RECEIPT: { "$exists": true } })
                    .build(),
            )
            .build()
    })
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(super) enum Receipt {
    Idle(RetainedReopenReceipt),
    Suspended(RetainedSuspendedReopenReceipt),
}

impl Receipt {
    pub(super) fn document(self, event_seq: u64) -> Result<Document, ApplyError> {
        let mut document = record_bson(&self)
            .map_err(SandboxStoreError::Corrupt)
            .and_then(|value| value.as_document().cloned().ok_or_else(corrupt))
            .map_err(ApplyError::Domain)?;
        let event_seq = i64::try_from(event_seq).map_err(|_| ApplyError::Domain(corrupt()))?;
        document.insert(EVENT_SEQ, event_seq);
        Ok(document)
    }
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("repeat retained recovery receipt is invalid".into())
}

fn event_receipt(
    row: &Document,
    parent: &Sandbox,
    last_seq: u64,
) -> Result<(RetainedReopenReplay, bool), SandboxStoreError> {
    let event = read_event(row)?;
    let mut envelope = row.get_document(RECEIPT).map_err(|_| corrupt())?.clone();
    let seq = i64::try_from(event.seq).map_err(|_| corrupt())?;
    if envelope.remove(EVENT_SEQ) != Some(mongodb::bson::Bson::Int64(seq)) {
        return Err(corrupt());
    }
    let receipt: Receipt =
        mongodb::bson::deserialize_from_document(envelope).map_err(|_| corrupt())?;
    let (replay, personal): (RetainedReopenReplay, bool) = match receipt {
        Receipt::Idle(receipt) => {
            transaction::validate_receipt(&receipt, parent)?;
            (receipt.into(), false)
        }
        Receipt::Suspended(receipt) => {
            suspended::validate_receipt(&receipt, parent, last_seq)?;
            if receipt.event_seq != event.seq {
                return Err(corrupt());
            }
            (receipt.into(), true)
        }
    };
    if event.sandbox_id != parent.id
        || event.seq == 0
        || event.seq > last_seq
        || event.appended_at != replay.accepted.accepted_at
        || !matches!(
            event.kind,
            SandboxEventKind::StatusChanged {
                from: SandboxStatus::Failed,
                to: SandboxStatus::Reviving
            }
        )
    {
        return Err(corrupt());
    }
    Ok((replay, personal))
}

pub(in crate::store) async fn read_for_service(
    store: &MongoSandboxStore,
    id: &SandboxId,
    key: Option<&str>,
) -> Result<RetainedReopenHistory, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(super::unavailable)?;
    let row = store
        .sandboxes
        .find_one(doc! { "_id": id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(super::unavailable)?
        .ok_or(SandboxStoreError::NotFound)?;
    read(
        store,
        &row,
        key,
        &mut session,
        Some(ReadConcern::snapshot()),
    )
    .await
    .map_err(ApplyError::domain)
}

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    row: &Document,
    key: Option<&str>,
    session: &mut ClientSession,
    read_concern: Option<ReadConcern>,
) -> Result<RetainedReopenHistory, ApplyError> {
    let domain = ApplyError::Domain;
    let options = FindOneOptions::builder().read_concern(read_concern).build();
    let parent = read_sandbox(row).map_err(domain)?;
    let observed_parent = Some((parent.owner.clone(), parent.revision));
    let (original, original_personal) = if row.contains_key(suspended::SUSPENDED_FIELD) {
        (suspended::pair(row).map_err(domain)?.map(Into::into), true)
    } else {
        (
            transaction::read_receipt(row)
                .map_err(domain)?
                .map(Into::into),
            false,
        )
    };
    let last_seq = row
        .get_i64("last_seq")
        .ok()
        .and_then(|seq| u64::try_from(seq).ok())
        .ok_or_else(|| domain(corrupt()))?;
    let latest = store
        .sandbox_events
        .find_one(doc! { "sandbox_id": parent.id.as_str(), RECEIPT: { "$exists": true } })
        .with_options(options.clone())
        .sort(doc! { REVISION: -1 })
        .hint(Hint::Name(REVISION_INDEX.into()))
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .map(|row| event_receipt(&row, &parent, last_seq))
        .transpose()
        .map_err(domain)?;
    let Some(original): Option<RetainedReopenReplay> = original else {
        return if latest.is_none() {
            Ok(RetainedReopenHistory {
                observed_parent,
                ..Default::default()
            })
        } else {
            Err(domain(corrupt()))
        };
    };
    if latest.as_ref().is_some_and(|(receipt, _)| {
        receipt.expected_revision <= original.accepted.revision
            || receipt.key_sha256 == original.key_sha256
    }) {
        return Err(domain(corrupt()));
    }
    let latest_revision = Some(
        latest
            .as_ref()
            .map_or(original.accepted.revision, |(receipt, _)| {
                receipt.accepted.revision
            }),
    );
    let matching = match key {
        Some(key) if key == original.key_sha256 => Some((original, original_personal)),
        Some(key)
            if latest
                .as_ref()
                .is_some_and(|(receipt, _)| receipt.key_sha256 == key) =>
        {
            latest
        }
        Some(key) if latest.is_some() => store
            .sandbox_events
            .find_one(doc! {
                "sandbox_id": parent.id.as_str(), RECEIPT: { "$exists": true }, KEY: key,
            })
            .with_options(options.clone())
            .hint(Hint::Name(KEY_INDEX.into()))
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .map(|row| {
                let (receipt, personal) = event_receipt(&row, &parent, last_seq)?;
                if receipt.key_sha256 != key
                    || receipt.expected_revision <= original.accepted.revision
                    || Some(receipt.accepted.revision) >= latest_revision
                {
                    return Err(corrupt());
                }
                Ok((receipt, personal))
            })
            .transpose()
            .map_err(domain)?,
        _ => None,
    };
    if matching.as_ref().is_some_and(|(_, personal)| *personal) {
        let policy = store
            .resource_policies
            .find_one(doc! { "_id": parent.id.as_str() })
            .with_options(options)
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        suspended::require_personal(policy.as_ref(), &parent).map_err(domain)?;
    }
    Ok(RetainedReopenHistory {
        observed_parent,
        latest_revision,
        replay: matching.map(|(receipt, _)| receipt),
    })
}

pub(super) async fn matches_event_receipt(
    store: &MongoSandboxStore,
    event: &SandboxEvent,
    receipt: Receipt,
    session: &mut ClientSession,
) -> Result<bool, SandboxStoreError> {
    let row = store
        .sandbox_events
        .find_one(doc! { "_id": event_id(&event.sandbox_id, event.seq) })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(super::unavailable)?;
    let expected = receipt.document(event.seq).map_err(ApplyError::domain)?;
    Ok(row.as_ref().and_then(|row| row.get_document(RECEIPT).ok()) == Some(&expected))
}
