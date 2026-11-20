//! Transactional lifecycle middleware and retained allocation usage ledger.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Sandbox, SandboxEvent};
use layer2_core::sandbox_usage::{
    observe, observe_capture_checkpoint, observe_funded_checkpoint, rebaseline, SandboxUsage,
    UsageEvent,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, IndexModel,
};

use super::sandbox_events::TransactionEventError;
use super::sandbox_mongo::{
    read_record, record_bson, signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

pub(super) const STATES: &str = "sandbox_usage";
pub(super) const EVENTS: &str = "sandbox_usage_events";

pub(crate) mod billing;
pub(in crate::store) mod checkpoint;
mod interval_index;
pub(in crate::store) mod raw_event;
mod reads;
pub(super) mod save;

pub(super) async fn ensure_indexes(
    store: &MongoSandboxStore,
) -> Result<(), layer2_core::StoreError> {
    store
        .sandbox_usage
        .create_index(
            IndexModel::builder()
                .keys(doc! { "owner": 1, "_id": 1 })
                .build(),
        )
        .await
        .map_err(|error| layer2_core::StoreError::Unavailable(format!("usage index: {error}")))?;
    store
        .sandbox_usage_events
        .create_indexes([
            IndexModel::builder()
                .keys(doc! { "sandbox_id": 1, "revision": 1 })
                .build(),
            IndexModel::builder()
                .keys(doc! { "owner": 1, "ended_at": 1 })
                .build(),
            IndexModel::builder()
                .keys(
                    doc! { "billing_delivered": 1, "billing_retry_at": 1, "ended_at": 1, "_id": 1 },
                )
                .build(),
        ])
        .await
        .map_err(|error| {
            layer2_core::StoreError::Unavailable(format!("usage event indexes: {error}"))
        })?;
    let database = store
        .client
        .database(&store.sandbox_usage_events.namespace().db);
    interval_index::ensure(&database).await?;
    Ok(())
}

pub(super) fn state_document(state: &SandboxUsage) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": state.sandbox_id.as_str(), "owner": state.owner.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": signed(state.revision, "usage revision").map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(state).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn event_document(event: &UsageEvent) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": event.transaction_id(), "sandbox_id": event.sandbox_id.as_str(),
        "owner": event.owner.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": signed(event.revision, "usage revision").map_err(SandboxStoreError::Corrupt)?,
        "started_at": signed(event.started_at, "usage start").map_err(SandboxStoreError::Corrupt)?,
        "ended_at": signed(event.ended_at, "usage end").map_err(SandboxStoreError::Corrupt)?,
        "billing_delivered": false,
        "billing_retry_at": 0_i64, "billing_lease_until": 0_i64,
        "record": record_bson(event).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read_state(document: &Document) -> Result<SandboxUsage, SandboxStoreError> {
    let state: SandboxUsage = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    let expected = state_document(&state)?;
    for key in ["_id", "owner", "revision"] {
        if document.get(key) != expected.get(key) {
            return Err(SandboxStoreError::Corrupt(
                "usage query fields disagree with record".into(),
            ));
        }
    }
    Ok(state)
}

pub(super) fn read_event(document: &Document) -> Result<UsageEvent, SandboxStoreError> {
    let event: UsageEvent = read_record(document).map_err(SandboxStoreError::Corrupt)?;
    let expected = event_document(&event)?;
    for key in [
        "_id",
        "owner",
        "sandbox_id",
        "revision",
        "started_at",
        "ended_at",
    ] {
        if document.get(key) != expected.get(key) {
            return Err(SandboxStoreError::Corrupt(
                "usage event query fields disagree with record".into(),
            ));
        }
    }
    Ok(event)
}

/// Called after the aggregate and event have been written, using their session.
/// No asynchronous send or HTTP request is part of lifecycle persistence.
pub(super) async fn lifecycle_event(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    event: &SandboxEvent,
    expected_revision: Option<u64>,
) -> Result<(), TransactionEventError> {
    let document = store
        .sandboxes
        .find_one(doc! { "_id": event.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(TransactionEventError::Driver)?
        .ok_or(TransactionEventError::Domain(SandboxStoreError::NotFound))?;
    let sandbox =
        super::sandboxes::read_sandbox(&document).map_err(TransactionEventError::Domain)?;
    record(
        store,
        session,
        &sandbox,
        event.seq,
        &event.appended_at,
        (
            event.seq.saturating_sub(1),
            expected_revision.unwrap_or(sandbox.revision),
        ),
        UsageObservation::Lifecycle,
    )
    .await
}

pub(super) enum UsageObservation {
    Lifecycle,
    Checkpoint,
    FundedCheckpoint,
    CaptureCheckpoint,
}

impl From<bool> for UsageObservation {
    fn from(checkpoint: bool) -> Self {
        if checkpoint {
            Self::Checkpoint
        } else {
            Self::Lifecycle
        }
    }
}

pub(super) async fn record(
    store: &MongoSandboxStore,
    session: &mut ClientSession,
    sandbox: &Sandbox,
    source_seq: u64,
    at: &str,
    expected_source: (u64, u64),
    mode: impl Into<UsageObservation>,
) -> Result<(), TransactionEventError> {
    let recorded = record_collections(
        (&store.sandbox_usage, &store.sandbox_usage_events),
        session,
        sandbox,
        source_seq,
        at,
        expected_source,
        mode,
    )
    .await?;
    if let (Some(capture), Some((state, event))) = (&store.usage_capture, recorded) {
        capture.record(session, &state, &event, sandbox).await?;
    }
    Ok(())
}

pub(super) async fn record_collections(
    collections: (
        &mongodb::Collection<Document>,
        &mongodb::Collection<Document>,
    ),
    session: &mut ClientSession,
    sandbox: &Sandbox,
    source_seq: u64,
    at: &str,
    expected_source: (u64, u64),
    mode: impl Into<UsageObservation>,
) -> Result<Option<(SandboxUsage, UsageEvent)>, TransactionEventError> {
    let current = collections
        .0
        .find_one(doc! { "_id": sandbox.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(TransactionEventError::Driver)?;
    let previous = current
        .as_ref()
        .map(read_state)
        .transpose()
        .map_err(TransactionEventError::Domain)?;
    // Compare the usage watermark with the aggregate BEFORE this write. Legacy
    // replicas can mutate it during the first rolling overlap; their missing
    // allocation edges must never be charged as one continuous known interval.
    let observation = match &previous {
        Some(previous) if (previous.source_seq, previous.source_revision) != expected_source => {
            rebaseline(previous, sandbox, source_seq, at)
        }
        _ => match mode.into() {
            UsageObservation::FundedCheckpoint => {
                observe_funded_checkpoint(previous.as_ref(), sandbox, source_seq, at)
            }
            UsageObservation::CaptureCheckpoint => {
                observe_capture_checkpoint(previous.as_ref(), sandbox, source_seq, at)
            }
            mode => observe(
                previous.as_ref(),
                sandbox,
                source_seq,
                at,
                matches!(mode, UsageObservation::Checkpoint),
            ),
        },
    }
    .map_err(|error| {
        TransactionEventError::Domain(SandboxStoreError::Corrupt(error.to_string()))
    })?;
    let Some((state, event)) = observation else {
        return Ok(None);
    };
    let document = state_document(&state).map_err(TransactionEventError::Domain)?;
    if previous.is_some() {
        collections
            .0
            .replace_one(doc! { "_id": sandbox.id.as_str() }, document)
            .session(&mut *session)
            .await
            .map_err(TransactionEventError::Driver)?;
    } else {
        collections
            .0
            .insert_one(document)
            .session(&mut *session)
            .await
            .map_err(TransactionEventError::Driver)?;
    }
    collections
        .1
        .insert_one(event_document(&event).map_err(TransactionEventError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(TransactionEventError::Driver)?;
    Ok(Some((state, event)))
}
