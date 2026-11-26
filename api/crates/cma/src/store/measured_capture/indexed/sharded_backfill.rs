//! Explicit generation handoff and resumable copying of immutable originals.
use super::{
    backfill::BACKFILL_ROWS,
    model::{
        digest, sealed_versioned, signed, valid_digest, Certification, Generation, Phase, ROOT_ID,
    },
    sharded_capacity::{DayDirectory, SHARDS},
    sharded_generation::{self as keys, ShardRoot, ShardedGeneration, VERSION},
    store_error, ActiveGeneration, CaptureError, IndexedCustody, TransactionError,
};
use crate::store::{
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
    transaction_terminal::GuardedTerminal as _,
};
use mongodb::{bson::doc, ClientSession};

impl IndexedCustody {
    pub(in crate::store::measured_capture) async fn begin_current(
        &self,
        expected_predecessor: Option<&str>,
    ) -> Result<(), CaptureError> {
        if expected_predecessor.is_some_and(|expected| !valid_digest(expected)) {
            return Err(CaptureError::Corrupt);
        }
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        if let Err(error) = self.begin_sharded(expected_predecessor, &mut session).await {
            let _ = session.abort_guarded().await;
            return Err(store_error(error));
        }
        commit(&mut session).await
    }

    async fn begin_sharded(
        &self,
        expected_predecessor: Option<&str>,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let present = self
            .manifests
            .find_one(doc! {"_id": ROOT_ID})
            .session(&mut *session)
            .await?
            .is_some();
        let predecessor = if present {
            match self.active_generation(session, false).await? {
                ActiveGeneration::Sharded(root) => {
                    if expected_predecessor.is_some()
                        && root.predecessor_sha256.as_deref() != expected_predecessor
                    {
                        return Err(CaptureError::Unavailable.into());
                    }
                    return Ok(());
                }
                ActiveGeneration::Legacy(_) if expected_predecessor.is_none() => return Ok(()),
                ActiveGeneration::Legacy(root) => {
                    if root.phase != Phase::Ready
                        || Some(digest(&root)?.as_str()) != expected_predecessor
                    {
                        return Err(CaptureError::Unavailable.into());
                    }
                    Some(root)
                }
            }
        } else {
            if expected_predecessor.is_some() {
                return Err(CaptureError::Unavailable.into());
            }
            None
        };
        let number = predecessor
            .as_ref()
            .map_or(Some(1), |p| p.generation.checked_add(1))
            .ok_or(CaptureError::Unavailable)?;
        let root = ShardedGeneration::new(
            number,
            self.writer_floor.clone(),
            expected_predecessor.map(str::to_owned),
        )?;
        for shard in 0..SHARDS {
            self.write_manifest(
                &keys::shard_id(number, shard),
                number,
                VERSION,
                None,
                &ShardRoot::new(number, shard)?,
                session,
            )
            .await?;
        }
        self.write_manifest(
            &keys::calendar_id(number),
            number,
            VERSION,
            None,
            &DayDirectory::default(),
            session,
        )
        .await?;
        match predecessor {
            Some(predecessor) => self.handoff(&predecessor, &root, session).await,
            None => {
                self.write_manifest(ROOT_ID, number, VERSION, None, &root, session)
                    .await?;
                Ok(())
            }
        }
    }

    async fn handoff(
        &self,
        predecessor: &Generation,
        root: &ShardedGeneration,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let previous = digest(predecessor)?;
        // Any in-flight legacy append must write this same exact authority. A
        // committed handoff fences it before the new original scan can start.
        let replaced = self
            .manifests
            .replace_one(
                doc! {"_id": ROOT_ID, "generation": signed(predecessor.generation)?,
                "schema_version": 1_i64, "payload_sha256": previous},
                sealed_versioned(ROOT_ID, root.generation, VERSION, root)?,
            )
            .session(&mut *session)
            .await?;
        if replaced.matched_count != 1 {
            return Err(CaptureError::Unavailable.into());
        }
        self.write_manifest(
            &keys::predecessor_id(root.generation),
            predecessor.generation,
            1,
            None,
            predecessor,
            session,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn advance_sharded(
        &self,
        mut root: ShardedGeneration,
        session: &mut ClientSession,
    ) -> Result<(u64, bool), TransactionError> {
        if root.phase == Phase::Ready {
            return Ok((root.backfilled_rows, true));
        }
        let previous = digest(&root)?;
        let filter = root
            .cursor
            .as_ref()
            .map_or_else(|| doc! {}, |id| doc! {"_id": {"$gt": id}});
        let mut cursor = self
            .originals
            .find(filter)
            .sort(doc! {"_id": 1})
            .limit(BACKFILL_ROWS)
            .session(&mut *session)
            .await?;
        let mut sources = Vec::new();
        while let Some(source) = cursor.next(&mut *session).await.transpose()? {
            sources.push(source);
        }
        drop(cursor);
        if sources.is_empty() {
            return Ok((root.backfilled_rows, true));
        }
        for source in sources {
            self.append_sharded_row(&root, &source, true, session)
                .await?;
            root.cursor = Some(
                source
                    .get_str("_id")
                    .map_err(|_| CaptureError::Corrupt)?
                    .into(),
            );
            root.backfilled_rows = root
                .backfilled_rows
                .checked_add(1)
                .ok_or(CaptureError::Unavailable)?;
        }
        self.publish_sharded(&mut root, &previous, session).await?;
        Ok((root.backfilled_rows, false))
    }

    pub(super) async fn finish_sharded(
        &self,
        root: ShardedGeneration,
        certification: Certification,
    ) -> Result<Certification, CaptureError> {
        let previous = digest(&root)?;
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let result = async {
            let mut current = match self.active_generation(&mut session, false).await? {
                ActiveGeneration::Sharded(current) if current == root => current,
                _ => return Err(CaptureError::Unavailable.into()),
            };
            current.phase = Phase::Ready;
            current.certification = Some(certification.clone());
            self.publish_sharded(&mut current, &previous, &mut session)
                .await
        }
        .await;
        if let Err(error) = result {
            let _ = session.abort_guarded().await;
            return Err(store_error(error));
        }
        commit(&mut session).await?;
        Ok(certification)
    }

    async fn publish_sharded(
        &self,
        root: &mut ShardedGeneration,
        previous: &str,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        root.revision = root
            .revision
            .checked_add(1)
            .ok_or(CaptureError::Unavailable)?;
        root.validate()?;
        self.write_manifest(
            ROOT_ID,
            root.generation,
            VERSION,
            Some(previous),
            root,
            session,
        )
        .await?;
        Ok(())
    }
}

async fn commit(session: &mut ClientSession) -> Result<(), CaptureError> {
    match commit_transaction(session).await {
        CommitOutcome::Committed => Ok(()),
        _ => Err(CaptureError::Unavailable),
    }
}
