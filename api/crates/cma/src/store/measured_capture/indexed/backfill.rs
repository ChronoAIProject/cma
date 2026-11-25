//! Resume a paused backfill, then independently certify source and manifest custody.
use super::{model::*, store_error, ActiveGeneration, IndexedCustody};
use crate::store::{
    measured_capture::{CaptureError, TransactionError},
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
    transaction_terminal::GuardedTerminal as _,
};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

pub(super) const BACKFILL_ROWS: i64 = 32;

impl IndexedCustody {
    #[cfg(test)]
    pub(super) async fn begin(&self) -> Result<(), CaptureError> {
        let root = Generation::new(1, self.writer_floor.clone())?;
        let document = sealed(ROOT_ID, root.generation, &root)?;
        let _ = self.manifests.insert_one(document).await;
        // An uncertain insert or concurrent begin is reconciled by the root.
        let mut session = self
            .database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        self.root(&mut session, true).await.map_err(store_error)?;
        Ok(())
    }

    pub(in crate::store::measured_capture) async fn advance(
        &self,
    ) -> Result<(u64, bool), CaptureError> {
        let mut session = self
            .database
            .client()
            .start_session()
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let result = match self.advance_inner(&mut session).await {
            Ok(result) => result,
            Err(error) => {
                let _ = session.abort_guarded().await;
                return Err(store_error(error));
            }
        };
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(result),
            _ => Err(CaptureError::Unavailable),
        }
    }

    async fn advance_inner(
        &self,
        session: &mut ClientSession,
    ) -> Result<(u64, bool), TransactionError> {
        let mut root = match self.active_generation(session, false).await? {
            ActiveGeneration::Legacy(root) => root,
            ActiveGeneration::Sharded(root) => {
                return self.advance_sharded(root, session).await;
            }
        };
        if root.phase == Phase::Ready {
            return Ok((root.rows, true));
        }
        let previous = digest(&root)?;
        let filter = root
            .cursor
            .as_ref()
            .map_or_else(|| doc! {}, |cursor| doc! {"_id": {"$gt": cursor}});
        let mut cursor = self
            .originals
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(BACKFILL_ROWS)
            .session(&mut *session)
            .await?;
        let mut rows = Vec::new();
        while let Some(source) = cursor.next(&mut *session).await.transpose()? {
            rows.push(Projection::from_source(root.generation, &source)?);
        }
        drop(cursor);
        if rows.is_empty() {
            return Ok((root.rows, true));
        }
        for projection in rows {
            self.insert_projection(&mut root, &projection, session)
                .await?;
            root.cursor = Some(projection.source_row_id);
        }
        self.publish_root(&mut root, &previous, session).await?;
        Ok((root.rows, false))
    }

    pub(in crate::store::measured_capture) async fn certify(
        &self,
    ) -> Result<Certification, CaptureError> {
        let mut snapshot = self
            .database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| CaptureError::Unavailable)?;
        let root = match self
            .active_generation(&mut snapshot, true)
            .await
            .map_err(store_error)?
        {
            ActiveGeneration::Legacy(root) => root,
            ActiveGeneration::Sharded(root) => {
                return self.certify_sharded(root, &mut snapshot).await;
            }
        };
        if root.phase == Phase::Ready {
            return root.certification.ok_or(CaptureError::Corrupt);
        }
        let certification = self
            .certify_rows(&root, &mut snapshot)
            .await
            .map_err(store_error)?;
        self.certify_directories(&root, &mut snapshot)
            .await
            .map_err(store_error)?;
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
            let mut current = self.root(&mut session, false).await?;
            if current != root {
                return Err(CaptureError::Unavailable.into());
            }
            current.phase = Phase::Ready;
            current.certification = Some(certification.clone());
            self.publish_root(&mut current, &previous, &mut session)
                .await
        }
        .await;
        if let Err(error) = result {
            let _ = session.abort_guarded().await;
            return Err(store_error(error));
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(certification),
            _ => Err(CaptureError::Unavailable),
        }
    }

    async fn certify_rows(
        &self,
        root: &Generation,
        session: &mut ClientSession,
    ) -> Result<Certification, TransactionError> {
        let mut originals = self
            .originals
            .find(doc! {})
            .sort(doc! {"_id":1})
            .read_concern(ReadConcern::snapshot())
            .max_time(Duration::from_secs(60))
            .session(&mut *session)
            .await?;
        // The projected stream is queried independently, including any stray generation.
        let mut projections = self
            .projections
            .find(doc! {})
            .sort(doc! {"_id":1})
            .read_concern(ReadConcern::snapshot())
            .max_time(Duration::from_secs(60))
            .session(&mut *session)
            .await?;
        let mut original_hash = Sha256::new();
        let mut projection_hash = Sha256::new();
        let mut count = 0_u64;
        while let Some(source) = originals.next(&mut *session).await.transpose()? {
            let expected = Projection::from_source(root.generation, &source)?;
            let row = projections
                .next(&mut *session)
                .await
                .transpose()?
                .ok_or(CaptureError::Corrupt)?;
            let actual = Projection::decode(&row)?;
            if expected != actual {
                return Err(CaptureError::Corrupt.into());
            }
            original_hash.update(expected.leaf()?.as_bytes());
            projection_hash.update(actual.leaf()?.as_bytes());
            count = count.checked_add(1).ok_or(CaptureError::Unavailable)?;
        }
        let ordered_sha256 = format!("{:x}", original_hash.finalize());
        if projections.next(&mut *session).await.transpose()?.is_some()
            || count != root.rows
            || ordered_sha256 != format!("{:x}", projection_hash.finalize())
        {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(Certification {
            original_rows: count,
            ordered_sha256,
        })
    }

    async fn certify_directories(
        &self,
        root: &Generation,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let mut count = 0_u64;
        for (day, expected) in &root.days {
            let day = canonical_number(day).ok_or(CaptureError::Corrupt)?;
            let directory: DayDirectory = self
                .manifest(
                    &day_id(root.generation, day),
                    root.generation,
                    Some(expected),
                    true,
                    session,
                )
                .await?;
            if directory.hours.is_empty() || directory.hours.len() > HOURS_PER_DAY as usize {
                return Err(CaptureError::Corrupt.into());
            }
            for hour in directory.hours.keys() {
                let hour = canonical_number(hour).ok_or(CaptureError::Corrupt)?;
                if hour / HOURS_PER_DAY != day {
                    return Err(CaptureError::Corrupt.into());
                }
                let directory = self.read_hour(root, hour, session).await?;
                let mut scopes = BTreeMap::<String, BTreeSet<String>>::new();
                for series in directory.series.keys() {
                    let mut budget = (0, 0);
                    let rows = self
                        .read_bucket(root, hour, series, &directory, session, &mut budget)
                        .await?;
                    if rows.is_empty() {
                        return Err(CaptureError::Corrupt.into());
                    }
                    for (projection, _) in rows {
                        scopes
                            .entry(projection.scope()?)
                            .or_default()
                            .insert(series.clone());
                        count = count.checked_add(1).ok_or(CaptureError::Unavailable)?;
                    }
                }
                if scopes.is_empty() || scopes != directory.scopes {
                    return Err(CaptureError::Corrupt.into());
                }
            }
        }
        if count != root.rows {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(())
    }
}
