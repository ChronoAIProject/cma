//! Update capacity authority in the same transaction as an admitted projection.

use super::model::{digest, open_versioned, signed, Phase, Projection};
use super::sharded_capacity::{DayDirectory, Permit, PermitDirectory, SamplePartition, SHARDS};
use super::sharded_generation::{
    calendar_id, permits_id, segment_id, shard_id, ShardRoot, ShardedGeneration, VERSION,
};
use super::{CaptureError, Document, IndexedCustody, TransactionError};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Segment {
    used: usize,
}

impl IndexedCustody {
    pub(super) async fn append_sharded(
        &self,
        generation: &ShardedGeneration,
        source: &Document,
        inserted: bool,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        if generation.phase != Phase::Ready {
            return Err(CaptureError::Unavailable.into());
        }
        self.append_sharded_row(generation, source, inserted, session)
            .await
    }

    pub(super) async fn append_sharded_row(
        &self,
        generation: &ShardedGeneration,
        source: &Document,
        inserted: bool,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let mut projection = Projection::from_source(generation.generation, source)?;
        let partition = SamplePartition::for_sample(&projection.keys.physical_sample_id);
        let id = shard_id(generation.generation, partition.preferred);
        let mut root: ShardRoot = self
            .authority(&id, generation.generation, false, session)
            .await?
            .ok_or(CaptureError::Corrupt)?;
        root.validate(generation.generation)?;
        if root.shard != partition.preferred {
            return Err(CaptureError::Corrupt.into());
        }
        if root.sealed {
            return Err(CaptureError::Unavailable.into());
        }
        if !inserted {
            let stored = self
                .verify_existing_projection(&root.days, &projection, Some(root.shard), session)
                .await?;
            let segment = stored.permit_segment.ok_or(CaptureError::Corrupt)?;
            let series = &stored.keys.physical_series_id;
            let directory: PermitDirectory = self
                .authority(
                    &permits_id(root.generation, stored.hour(), series),
                    root.generation,
                    false,
                    session,
                )
                .await?
                .ok_or(CaptureError::Corrupt)?;
            let usage: Option<Segment> = self
                .authority(
                    &segment_id(root.generation, stored.hour(), series, segment),
                    root.generation,
                    false,
                    session,
                )
                .await?;
            if matches!(
                directory.selected(segment, usage.map(|row| row.used))?,
                Some(Permit::Register(_))
            ) {
                return Err(CaptureError::Corrupt.into());
            }
            return Ok(());
        }
        let previous = digest(&root)?;
        self.register_sharded_day(generation.generation, projection.day(), session)
            .await?;
        projection.permit_segment = Some(
            self.consume_series_permit(
                generation.generation,
                projection.hour(),
                &projection.keys.physical_series_id,
                &partition,
                session,
            )
            .await?,
        );
        self.insert_projection_at(&mut root.days, &projection, Some(root.shard), session)
            .await?;
        root.rows = root.rows.checked_add(1).ok_or(CaptureError::Unavailable)?;
        root.revision = root
            .revision
            .checked_add(1)
            .ok_or(CaptureError::Unavailable)?;
        self.write_manifest(
            &id,
            generation.generation,
            VERSION,
            Some(&previous),
            &root,
            session,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn authority<T: Serialize + DeserializeOwned>(
        &self,
        id: &str,
        generation: u64,
        snapshot: bool,
        session: &mut ClientSession,
    ) -> Result<Option<T>, TransactionError> {
        let mut action = self.manifests.find_one(doc! {"_id": id});
        if snapshot {
            action = action.read_concern(ReadConcern::snapshot());
        }
        action
            .session(session)
            .await?
            .map(|row| open_versioned(&row, id, generation, VERSION, None).map_err(Into::into))
            .transpose()
    }

    pub(super) async fn register_sharded_day(
        &self,
        generation: u64,
        day: u64,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let id = calendar_id(generation);
        let mut calendar: DayDirectory = self
            .authority(&id, generation, false, session)
            .await?
            .ok_or(CaptureError::Corrupt)?;
        let previous = digest(&calendar)?;
        if calendar.register(day)? {
            self.write_manifest(
                &id,
                generation,
                VERSION,
                Some(&previous),
                &calendar,
                session,
            )
            .await?;
        }
        Ok(())
    }

    pub(super) async fn consume_series_permit(
        &self,
        generation: u64,
        hour: u64,
        series: &str,
        partition: &SamplePartition,
        session: &mut ClientSession,
    ) -> Result<u8, TransactionError> {
        let id = permits_id(generation, hour, series);
        let existing: Option<PermitDirectory> =
            self.authority(&id, generation, false, session).await?;
        if existing.is_none() {
            // A previously spent series cannot initialize a new registry
            // after losing capacity records.
            let has_projection = self.projections
                .find_one(doc! {"generation": signed(generation)?, "hour": signed(hour)?, "series": series})
                .session(&mut *session)
                .await?
                .is_some();
            if has_projection
                || !self
                    .read_permit_segments(generation, hour, series, false, session)
                    .await?
                    .is_empty()
            {
                return Err(CaptureError::Corrupt.into());
            }
        }
        let previous = existing.as_ref().map(digest).transpose()?;
        let mut directory = existing.unwrap_or_default();
        let preferred = segment_id(generation, hour, series, partition.preferred);
        let observed: Option<Segment> = self
            .authority(&preferred, generation, false, session)
            .await?;
        let permit =
            match directory.selected(partition.preferred, observed.map(|record| record.used))? {
                Some(permit) => permit,
                None => {
                    let rows = self
                        .read_permit_segments(generation, hour, series, false, session)
                        .await?;
                    directory
                        .alternative(partition.fallback, &rows)?
                        .ok_or(CaptureError::Unavailable)?
                }
            };
        let (slot, previous_segment, next) = match permit {
            Permit::Register(slot) => {
                directory.register(slot)?;
                self.write_manifest(
                    &id,
                    generation,
                    VERSION,
                    previous.as_deref(),
                    &directory,
                    session,
                )
                .await?;
                (slot, None, Segment { used: 1 })
            }
            Permit::Spend { segment, used } => (
                segment,
                Some(digest(&Segment { used })?),
                Segment { used: used + 1 },
            ),
        };
        self.write_manifest(
            &segment_id(generation, hour, series, slot),
            generation,
            VERSION,
            previous_segment.as_deref(),
            &next,
            session,
        )
        .await?;
        Ok(slot)
    }

    pub(super) async fn read_permit_segments(
        &self,
        generation: u64,
        hour: u64,
        series: &str,
        snapshot: bool,
        session: &mut ClientSession,
    ) -> Result<BTreeMap<u8, usize>, TransactionError> {
        let ids: BTreeMap<_, _> = (0..SHARDS)
            .map(|slot| (segment_id(generation, hour, series, slot), slot))
            .collect();
        let selected: Vec<_> = ids.keys().map(String::as_str).collect();
        let mut action = self.manifests.find(doc! {"_id": {"$in": selected}});
        if snapshot {
            action = action.read_concern(ReadConcern::snapshot());
        }
        let mut cursor = action.session(&mut *session).await?;
        let mut usage = BTreeMap::new();
        while let Some(row) = cursor.next(&mut *session).await.transpose()? {
            let id = row.get_str("_id").map_err(|_| CaptureError::Corrupt)?;
            let slot = *ids.get(id).ok_or(CaptureError::Corrupt)?;
            let segment: Segment = open_versioned(&row, id, generation, VERSION, None)?;
            if usage.insert(slot, segment.used).is_some() {
                return Err(CaptureError::Corrupt.into());
            }
        }
        Ok(usage)
    }
}
