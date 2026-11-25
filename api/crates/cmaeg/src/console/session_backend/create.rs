//! Admission and quota eviction share one short transaction, serialized by a
//! sentinel. No network call outside Mongo is made while this transaction lives.
use super::{
    mongo::{binary, bounded, live_all, stored},
    schema::QUOTA,
    *,
};
use crate::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, DateTime, Document},
    options::ReturnDocument,
    ClientSession,
};

#[derive(Debug)]
enum AttemptError {
    Mongo(mongodb::error::Error),
    Store(StoreError),
}
impl From<mongodb::error::Error> for AttemptError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Mongo(value)
    }
}
impl From<StoreError> for AttemptError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl MongoSessionBackend {
    pub(super) async fn create_session(
        &self,
        id: &str,
        subject: &str,
        payload: Vec<u8>,
        access_ttl: i64,
        limits: &SessionLimits,
    ) -> Result<Stored, StoreError> {
        let ttl = i64::try_from(limits.ttl.as_millis()).map_err(|_| StoreError::Invalid)?;
        let idle = i64::try_from(limits.idle.as_millis()).map_err(|_| StoreError::Invalid)?;
        if !identifier(id)
            || !identifier(subject)
            || !payload_valid(&payload)
            || !(1..=86_400_000).contains(&access_ttl)
            || ttl <= 0
            || idle <= 0
            || limits.max == 0
            || limits.max > MAX_SESSIONS
            || limits.per_subject == 0
            || limits.per_subject > limits.max
        {
            return Err(StoreError::Invalid);
        }
        let result = bounded("create", async {
            for attempt in 0..5 {
                let mut session = self
                    .db
                    .client()
                    .start_session()
                    .await
                    .map_err(|_| StoreError::Unavailable)?;
                crate::transaction_start::start_write(&self.db, &mut session, Some(SERVER_TIMEOUT))
                    .await
                    .map_err(|_| StoreError::Unavailable)?;
                let result = self
                    .insert_session(
                        &mut session,
                        id,
                        subject,
                        &payload,
                        access_ttl,
                        (ttl, idle),
                        limits,
                    )
                    .await;
                match result {
                    Ok(row) => match session.commit_guarded().await {
                        Ok(()) => return Ok(row),
                        Err(error) if known_abort(&error) && attempt < 4 => {}
                        Err(_) => return Err(StoreError::Unavailable),
                    },
                    Err(error) => {
                        // An abort is cleanup, never evidence that an uncertain
                        // commit failed. This branch has not attempted commit.
                        if session.abort_guarded().await.is_err() {
                            tracing::warn!(
                                "console session transaction abort acknowledgement unavailable"
                            );
                        }
                        match error {
                            AttemptError::Mongo(error) if known_abort(&error) && attempt < 4 => {}
                            AttemptError::Store(error) => return Err(error),
                            AttemptError::Mongo(_) => return Err(StoreError::Unavailable),
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(5 << attempt)).await;
            }
            Err(StoreError::Unavailable)
        })
        .await;
        match result {
            Ok(row) => Ok(row),
            Err(StoreError::Invalid) => Err(StoreError::Invalid),
            Err(StoreError::Unavailable) => {
                // The same random id and encrypted revision identify this
                // admission exactly. Never mint a replacement after uncertainty.
                match self.load(id, false).await? {
                    Some(row) if row.payload == payload => Ok(row),
                    _ => Err(StoreError::Unavailable),
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_session(
        &self,
        session: &mut ClientSession,
        id: &str,
        subject: &str,
        payload: &[u8],
        access_ttl: i64,
        (ttl, idle): (i64, i64),
        limits: &SessionLimits,
    ) -> Result<Stored, AttemptError> {
        let quota = self
            .rows
            .find_one_and_update(
                doc! {"_id":QUOTA,"kind":"control","schema_version":1_i32,"fence":{"$lt":i64::MAX}},
                vec![doc! {"$set":{"fence":{"$add":["$fence",1_i64]},"observed_at":"$$NOW"}}],
            )
            .return_document(ReturnDocument::After)
            .max_time(SERVER_TIMEOUT)
            .session(&mut *session)
            .await?
            .ok_or(StoreError::Unavailable)?;
        let now = quota
            .get_datetime("observed_at")
            .map_err(|_| StoreError::Invalid)?
            .timestamp_millis();
        let date = |delta| {
            now.checked_add(delta)
                .map(DateTime::from_millis)
                .ok_or(StoreError::Invalid)
        };
        let absolute = date(ttl)?;
        let mut subject_filter = live_all();
        subject_filter.insert("subject_hash", subject);
        let mut evicted = Vec::new();
        self.evict(
            session,
            subject_filter,
            doc! {"created_at":1,"_id":1},
            limits.per_subject,
            &mut evicted,
        )
        .await?;
        self.evict(
            session,
            live_all(),
            doc! {"last_seen":1,"_id":1},
            limits.max,
            &mut evicted,
        )
        .await?;
        let row = doc! {"_id":id,"kind":"session","subject_hash":subject,"payload":binary(payload.to_vec()),
        "created_at":DateTime::from_millis(now),"last_seen":DateTime::from_millis(now),
        "observed_at":DateTime::from_millis(now),"absolute_at":absolute,
        "expires_at":date(ttl.min(idle))?,"access_expires_at":date(access_ttl)?,"idle_ms":idle,"fence":0_i64};
        self.rows
            .insert_one(row.clone())
            .session(&mut *session)
            .await?;
        let mut stored = stored(row)?;
        stored.evicted = evicted;
        Ok(stored)
    }

    async fn evict(
        &self,
        session: &mut ClientSession,
        filter: Document,
        sort: Document,
        cap: usize,
        victims: &mut Vec<Evicted>,
    ) -> Result<(), AttemptError> {
        let count = self
            .rows
            .count_documents(filter.clone())
            .max_time(SERVER_TIMEOUT)
            .session(&mut *session)
            .await?;
        let excess = count.saturating_add(1).saturating_sub(cap as u64);
        if excess == 0 {
            return Ok(());
        }
        // Bound even a corrupt/unexpectedly oversized collection; fail closed
        // instead of collecting an unbounded eviction list in application RAM.
        if excess > MAX_SESSIONS as u64 {
            return Err(StoreError::Unavailable.into());
        }
        let mut cursor = self
            .rows
            .find(filter)
            .sort(sort)
            .limit(excess as i64)
            .projection(doc! {"_id":1})
            .batch_size(256)
            .max_time(SERVER_TIMEOUT)
            .session(&mut *session)
            .await?;
        let mut ids = Vec::with_capacity(excess as usize);
        while let Some(row) = cursor.stream(&mut *session).try_next().await? {
            let id = row
                .get_str("_id")
                .map_err(|_| StoreError::Invalid)?
                .to_owned();
            ids.push(id);
        }
        let cleanup_ids: Vec<_> = ids
            .iter()
            .take(4_usize.saturating_sub(victims.len()))
            .cloned()
            .collect();
        if !cleanup_ids.is_empty() {
            let mut cleanup = self
                .rows
                .find(doc! {"kind":"session","_id":{"$in":cleanup_ids}})
                .projection(doc! {"_id":1,"payload":1})
                .limit(4)
                .batch_size(4)
                .max_time(SERVER_TIMEOUT)
                .session(&mut *session)
                .await?;
            while let Some(row) = cleanup.stream(&mut *session).try_next().await? {
                let id = row
                    .get_str("_id")
                    .map_err(|_| StoreError::Invalid)?
                    .to_owned();
                let payload = row
                    .get_binary_generic("payload")
                    .map_err(|_| StoreError::Invalid)?
                    .clone();
                if !payload_valid(&payload) {
                    return Err(StoreError::Invalid.into());
                }
                victims.push(Evicted { id, payload });
            }
        }
        self.rows
            .delete_many(doc! {"kind":"session","_id":{"$in":ids}})
            .session(&mut *session)
            .await?;
        tracing::info!(evicted = excess, "console session quota enforced");
        Ok(())
    }
}

pub(super) fn known_abort(error: &mongodb::error::Error) -> bool {
    error.contains_label("TransientTransactionError")
        && !error.contains_label("UnknownTransactionCommitResult")
}
