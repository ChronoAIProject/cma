//! Replica-mode desktop claims share the Sandbox's maintenance write fence.
use layer2_core::sandbox::store::{DesktopSessionRequest, SandboxStoreError};
use layer3_adapter::DesktopOwnership;
use mongodb::{
    bson::{doc, Document},
    options::ReturnDocument,
    ClientSession,
};
use std::time::{Duration, UNIX_EPOCH};

use super::super::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION,
};

enum Error {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

pub(super) async fn claim(
    store: &MongoSandboxStore,
    request: &DesktopSessionRequest,
    previous: Option<&DesktopOwnership>,
) -> Result<DesktopOwnership, SandboxStoreError> {
    if request.sandbox_id.as_str().trim().is_empty()
        || request.session_id.trim().is_empty()
        || request.session_id.len() > super::MAX_SESSION_ID_BYTES
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "invalid desktop claim request",
        ));
    }
    if let Some(previous) = previous {
        previous
            .validate()
            .map_err(|_| SandboxStoreError::Conflict)?;
        if previous.sandbox_id != request.sandbox_id.as_str()
            || previous.session_id != request.session_id
            || previous.runtime_id != request.runtime.runtime_id
            || previous.boot_id != request.runtime.boot_id
        {
            return Err(SandboxStoreError::Conflict);
        }
    }
    // The receiver challenge bounds the caller's whole operation as well.
    tokio::time::timeout(
        Duration::from_secs(3),
        transaction(store, request, previous),
    )
    .await
    .map_err(|_| unavailable())?
}

async fn transaction(
    store: &MongoSandboxStore,
    request: &DesktopSessionRequest,
    previous: Option<&DesktopOwnership>,
) -> Result<DesktopOwnership, SandboxStoreError> {
    for attempt in 0..8 {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|_| unavailable())?;
        start_transaction(&mut session)
            .await
            .map_err(|_| unavailable())?;
        let result = apply(store, request, previous, &mut session).await;
        match result {
            Ok(authority) => match commit_transaction(&mut session).await {
                CommitOutcome::Committed => {
                    tracing::debug!(renewal = previous.is_some(), "desktop claim committed");
                    return Ok(authority);
                }
                CommitOutcome::Retry(_) => {}
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    // Never replay an uncertain acquisition. An unobserved claim
                    // expires; no runtime socket is opened without acknowledged authority.
                    tracing::warn!("desktop claim commit uncertain or unavailable");
                    return Err(unavailable());
                }
            },
            Err(error) => {
                if session.abort_transaction().await.is_err() {
                    tracing::warn!("desktop claim transaction abort unavailable");
                }
                match error {
                    Error::Domain(error) => return Err(error),
                    Error::Driver(error) if is_transient(&error) => {}
                    Error::Driver(_) => return Err(unavailable()),
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(1 << attempt.min(5))).await;
    }
    Err(SandboxStoreError::Conflict)
}

async fn apply(
    store: &MongoSandboxStore,
    request: &DesktopSessionRequest,
    previous: Option<&DesktopOwnership>,
    session: &mut ClientSession,
) -> Result<DesktopOwnership, Error> {
    let runtime = mongodb::bson::serialize_to_bson(&request.runtime)
        .map_err(|_| Error::Domain(SandboxStoreError::Conflict))?;
    let mut increments = doc! { "desktop_activity_fence": 1_i64 };
    if previous.is_none() {
        increments.insert("desktop_session_epoch", 1_i64);
    }
    // Both session admission and maintenance write this document in their
    // snapshot transaction. A phantom session cannot slip between proof and pause.
    let row = store
        .sandboxes
        .find_one_and_update(
            doc! {
                "_id": request.sandbox_id.as_str(), "status": "active",
                "document_schema_version": DOCUMENT_SCHEMA_VERSION, "record.runtime": runtime,
            },
            doc! { "$inc": increments },
        )
        .projection(doc! { "desktop_session_epoch": 1, "authority_now": "$$NOW" })
        .return_document(ReturnDocument::After)
        .session(&mut *session)
        .await
        .map_err(Error::Driver)?
        .ok_or(Error::Domain(SandboxStoreError::Conflict))?;
    let epoch = row
        .get_i64("desktop_session_epoch")
        .ok()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or(Error::Domain(SandboxStoreError::Conflict))?;
    let issued_at_ms = row
        .get_datetime("authority_now")
        .map_err(|_| Error::Domain(unavailable()))?
        .timestamp_millis();
    let expires_at_ms = (issued_at_ms / 1000 + 15) * 1000;
    let authority = DesktopOwnership {
        sandbox_id: request.sandbox_id.to_string(),
        session_id: request.session_id.clone(),
        runtime_id: request.runtime.runtime_id.clone(),
        boot_id: request.runtime.boot_id.clone(),
        epoch,
        issued_at_ms,
        expires_at_ms,
    };
    authority
        .validate()
        .map_err(|_| Error::Domain(SandboxStoreError::Conflict))?;
    let now = stamp(issued_at_ms);
    let expiry = stamp(expires_at_ms);
    if let Some(previous) = previous {
        if epoch != previous.epoch {
            return Err(Error::Domain(SandboxStoreError::Conflict));
        }
        let mut filter = exact(previous);
        filter.insert("expires_at_ms", previous.expires_at_ms);
        filter.insert("lease_expires_at", doc! { "$gt": &now });
        let changed = store
            .sandbox_sessions
            .update_one(
                filter,
                doc! { "$set": {
                    "issued_at_ms": issued_at_ms, "expires_at_ms": expires_at_ms,
                    "lease_expires_at": expiry,
                } },
            )
            .session(&mut *session)
            .await
            .map_err(Error::Driver)?;
        if changed.matched_count != 1 {
            return Err(Error::Domain(SandboxStoreError::Conflict));
        }
    } else {
        let live = store
            .sandbox_sessions
            .find_one(doc! {
                "sandbox_id": &authority.sandbox_id, "lease_expires_at": { "$gt": &now },
            })
            .session(&mut *session)
            .await
            .map_err(Error::Driver)?;
        if live.is_some() {
            return Err(Error::Domain(SandboxStoreError::Conflict));
        }
        // The permanent epoch is on the Sandbox; expired session rows are only
        // disposable activity evidence and can be reclaimed without resetting it.
        store
            .sandbox_sessions
            .delete_many(doc! {
                "sandbox_id": &authority.sandbox_id, "lease_expires_at": { "$lte": &now },
            })
            .session(&mut *session)
            .await
            .map_err(Error::Driver)?;
        let mut document = exact(&authority);
        document.insert("issued_at_ms", issued_at_ms);
        document.insert("expires_at_ms", expires_at_ms);
        document.insert("lease_expires_at", expiry);
        store
            .sandbox_sessions
            .insert_one(document)
            .session(session)
            .await
            .map_err(Error::Driver)?;
    }
    Ok(authority)
}

pub(super) async fn release(
    store: &MongoSandboxStore,
    authority: &DesktopOwnership,
) -> Result<(), SandboxStoreError> {
    authority
        .validate()
        .map_err(|_| SandboxStoreError::Conflict)?;
    store
        .sandbox_sessions
        .delete_one(exact(authority))
        .write_concern(mongodb::options::WriteConcern::majority())
        .await
        .map_err(|_| unavailable())?;
    Ok(())
}

fn exact(authority: &DesktopOwnership) -> Document {
    doc! {
        "_id": &authority.session_id, "sandbox_id": &authority.sandbox_id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION, "epoch": authority.epoch as i64,
        "runtime_id": &authority.runtime_id, "boot_id": &authority.boot_id,
    }
}

fn stamp(millis: i64) -> String {
    layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_millis(millis.max(0) as u64))
}

fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("shared desktop session authority unavailable".into())
}

#[cfg(test)]
mod mongo_tests;

#[cfg(test)]
mod service_tests;
