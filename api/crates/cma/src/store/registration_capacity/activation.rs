//! The immutable cohort marker also admits the new Launch reservation format.

use crate::store::transaction_terminal::GuardedTerminal as _;
use mongodb::{
    bson::{doc, Document},
    Client, ClientSession, Collection,
};

pub(crate) enum Error {
    Driver(mongodb::error::Error),
    Unreadable(layer2_core::StoreError),
}

impl Error {
    pub(in crate::store) fn into_store_error(self) -> layer2_core::StoreError {
        match self {
            Self::Driver(error) => {
                super::unavailable(&format!("read capacity activation: {error}"))
            }
            Self::Unreadable(error) => error,
        }
    }
}

pub(crate) async fn read(
    admissions: &Collection<Document>,
    session: Option<&mut ClientSession>,
) -> Result<bool, Error> {
    let query = admissions
        .find_one(super::capacity_key())
        .max_time(super::QUERY_TIME);
    let marker = match session {
        Some(session) => query.session(session).await,
        None => query.await,
    }
    .map_err(Error::Driver)?;
    if let Some(marker) = marker {
        super::registration_count(&marker).map_err(Error::Unreadable)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// First activation and preactivation admissions share settings -> cohort order.
pub(super) async fn activate(
    client: &Client,
    admissions: &Collection<Document>,
    settings: &Collection<Document>,
    initial: crate::store::settings_defaults::InitialSettings,
) -> Result<(), layer2_core::StoreError> {
    use crate::store::sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    };
    if read(admissions, None)
        .await
        .map_err(Error::into_store_error)?
    {
        return Ok(());
    }
    for attempt in 0..super::ATTEMPTS {
        crate::store::contention::backoff(attempt).await;
        let mut session = client
            .start_session()
            .await
            .map_err(|error| super::unavailable(&format!("start activation session: {error}")))?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                continue;
            }
            return Err(super::unavailable(&format!("start activation: {error}")));
        }
        match activate_once(admissions, settings, initial, &mut session).await {
            Ok(()) => match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(error) => {
                    return Err(super::unavailable(&format!(
                        "activation commit outcome is unknown: {error}"
                    )))
                }
                CommitOutcome::Failed(error) => {
                    return Err(super::unavailable(&format!("commit activation: {error}")))
                }
            },
            Err(error) => {
                let _ = session.abort_guarded().await;
                if matches!(&error, Error::Driver(error) if is_transient(error) || is_duplicate_key(error))
                {
                    continue;
                }
                return Err(error.into_store_error());
            }
        }
    }
    Err(super::unavailable(
        "registration activation contention exhausted the retry budget",
    ))
}

async fn activate_once(
    admissions: &Collection<Document>,
    settings: &Collection<Document>,
    initial: crate::store::settings_defaults::InitialSettings,
    session: &mut ClientSession,
) -> Result<(), Error> {
    crate::store::settings_barrier::acquire(settings, initial, session)
        .await
        .map_err(|error| match error {
            crate::store::settings_barrier::Error::Driver(error) => Error::Driver(error),
            crate::store::settings_barrier::Error::Unreadable => {
                Error::Unreadable(super::unavailable("activation settings are unreadable"))
            }
        })?;
    if read(admissions, Some(&mut *session)).await? {
        return Ok(());
    }
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339())
        .and_then(|now| i64::try_from(now).ok())
        .ok_or_else(|| {
            Error::Unreadable(super::unavailable(
                "registration activation clock is unavailable",
            ))
        })?;
    let mut document = super::capacity_key();
    document.extend(doc! {
        "document_schema_version": crate::store::schema::DOCUMENT_SCHEMA_VERSION,
        "count": 0_i64, "activated_at": now,
    });
    admissions
        .insert_one(document)
        .session(session)
        .await
        .map_err(Error::Driver)?;
    Ok(())
}
