//! Independently certify source order, directory membership, and capacity.
use super::{
    model::{canonical_number, open, signed, Certification, Generation, Phase, Projection},
    sharded_capacity::SamplePartition,
    sharded_generation::{predecessor_id, ShardedGeneration},
    store_error, CaptureError, IndexedCustody, TransactionError,
};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

impl IndexedCustody {
    pub(super) async fn certify_sharded(
        &self,
        root: ShardedGeneration,
        snapshot: &mut ClientSession,
    ) -> Result<Certification, CaptureError> {
        if root.phase == Phase::Ready {
            return root.certification.ok_or(CaptureError::Corrupt);
        }
        let certification = self
            .certify_sharded_rows(&root, snapshot)
            .await
            .map_err(store_error)?;
        self.certify_sharded_directories(&root, snapshot)
            .await
            .map_err(store_error)?;
        self.finish_sharded(root, certification).await
    }

    async fn certify_sharded_rows(
        &self,
        root: &ShardedGeneration,
        snapshot: &mut ClientSession,
    ) -> Result<Certification, TransactionError> {
        let mut originals = self
            .originals
            .find(doc! {})
            .sort(doc! {"_id": 1})
            .read_concern(ReadConcern::snapshot())
            .max_time(Duration::from_secs(60))
            .session(&mut *snapshot)
            .await?;
        let mut projections = self.projections
            .find(doc! {"$or": [
                {"generation": signed(root.generation)?},
                {"_id": {"$gte": format!("{}:", root.generation), "$lt": format!("{};", root.generation)}},
            ]})
            .sort(doc! {"_id": 1})
            .read_concern(ReadConcern::snapshot())
            .max_time(Duration::from_secs(60))
            .session(&mut *snapshot)
            .await?;
        let mut expected = Sha256::new();
        let mut observed = Sha256::new();
        let mut count = 0_u64;
        while let Some(original) = originals.next(&mut *snapshot).await.transpose()? {
            let row = projections
                .next(&mut *snapshot)
                .await
                .transpose()?
                .ok_or(CaptureError::Corrupt)?;
            let source = Projection::from_source(root.generation, &original)?;
            let mut projected = Projection::decode(&row)?;
            if projected.permit_segment.take().is_none() || source != projected {
                return Err(CaptureError::Corrupt.into());
            }
            expected.update(source.leaf()?.as_bytes());
            observed.update(projected.leaf()?.as_bytes());
            count = count.checked_add(1).ok_or(CaptureError::Unavailable)?;
        }
        let ordered_sha256 = format!("{:x}", expected.finalize());
        if projections
            .next(&mut *snapshot)
            .await
            .transpose()?
            .is_some()
            || count != root.backfilled_rows
            || ordered_sha256 != format!("{:x}", observed.finalize())
        {
            return Err(CaptureError::Corrupt.into());
        }
        if let Some(predecessor) = &root.predecessor_sha256 {
            let id = predecessor_id(root.generation);
            let row = self
                .manifests
                .find_one(doc! {"_id": &id})
                .read_concern(ReadConcern::snapshot())
                .session(&mut *snapshot)
                .await?
                .ok_or(CaptureError::Corrupt)?;
            let previous: Generation = open(&row, &id, root.generation - 1, Some(predecessor))?;
            previous.validate()?;
            if previous.phase != Phase::Ready
                || previous.rows != count
                || previous.generation != root.generation - 1
                || previous.writer_floor_sha256 != root.writer_floor_sha256
            {
                return Err(CaptureError::Corrupt.into());
            }
        }
        Ok(Certification {
            original_rows: count,
            ordered_sha256,
        })
    }

    async fn certify_sharded_directories(
        &self,
        root: &ShardedGeneration,
        snapshot: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let roots = self.read_shard_roots(root, snapshot, &mut 0).await?;
        let days: BTreeSet<_> = roots
            .values()
            .flat_map(|shard| shard.days.keys())
            .map(|value| canonical_number(value).ok_or(CaptureError::Corrupt))
            .collect::<Result<_, _>>()?;
        let mut counts = BTreeMap::<u8, u64>::new();
        for day in days {
            let directories = self
                .read_shard_days(root.generation, &roots, day, snapshot, &mut 0)
                .await?;
            let hours: BTreeSet<_> = directories
                .values()
                .flat_map(|directory| directory.hours.keys())
                .map(|value| canonical_number(value).ok_or(CaptureError::Corrupt))
                .collect::<Result<_, _>>()?;
            for hour in hours {
                let directories = self
                    .read_shard_hour_documents(
                        root.generation,
                        hour,
                        &directories,
                        snapshot,
                        &mut 0,
                    )
                    .await?;
                let series: BTreeSet<_> = directories
                    .values()
                    .flat_map(|dir| dir.series.keys())
                    .collect();
                let mut scopes = BTreeMap::<u8, BTreeMap<String, BTreeSet<String>>>::new();
                for series in series {
                    let rows = self
                        .read_shard_bucket(
                            root.generation,
                            hour,
                            series,
                            &directories,
                            snapshot,
                            &mut 0,
                            &mut (0, 0),
                        )
                        .await?;
                    if rows.is_empty() {
                        return Err(CaptureError::Corrupt.into());
                    }
                    for (point, _) in rows {
                        let shard =
                            SamplePartition::for_sample(&point.keys.physical_sample_id).preferred;
                        let count = counts.entry(shard).or_default();
                        *count = count.checked_add(1).ok_or(CaptureError::Unavailable)?;
                        scopes
                            .entry(shard)
                            .or_default()
                            .entry(point.scope()?)
                            .or_default()
                            .insert(series.clone());
                    }
                }
                for (shard, directory) in directories {
                    if directory.scopes.is_empty()
                        || scopes.remove(&shard).as_ref() != Some(&directory.scopes)
                    {
                        return Err(CaptureError::Corrupt.into());
                    }
                }
                if !scopes.is_empty() {
                    return Err(CaptureError::Corrupt.into());
                }
            }
        }
        let observed = counts
            .values()
            .try_fold(0_u64, |count, add| count.checked_add(*add))
            .ok_or(CaptureError::Unavailable)?;
        if observed != root.backfilled_rows
            || roots
                .values()
                .any(|root| counts.get(&root.shard).copied().unwrap_or_default() != root.rows)
        {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(())
    }
}
