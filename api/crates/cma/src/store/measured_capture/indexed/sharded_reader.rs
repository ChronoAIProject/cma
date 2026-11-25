//! Check shard membership, discovery, and exact capacity in one read snapshot.
use super::{
    model::{
        canonical_number, open, open_versioned, DayDirectory, HourDirectory, Projection,
        SeriesManifest, HOURS_PER_DAY, HOUR_MS, MAX_SERIES_HOUR_ROWS,
    },
    reader::{BucketQuery, Positions},
    sharded_capacity::{DayDirectory as Calendar, PermitDirectory, SamplePartition, SHARDS},
    sharded_generation as keys, CaptureError, Document, IndexedCustody, TransactionError,
};
use crate::store::measured_capture::preview::{retain, MAX_ROWS};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
    time::Duration,
};

const MAX_MANIFEST_BYTES: usize = 48 * 1024 * 1024;
pub(super) type Roots = BTreeMap<u8, keys::ShardRoot>;
pub(super) type Hours = BTreeMap<u8, HourDirectory>;

struct ManifestRef {
    shard: u8,
    id: String,
    digest: Option<String>,
}

impl IndexedCustody {
    pub(super) async fn read_sharded_range(
        &self,
        root: &keys::ShardedGeneration,
        range: RangeInclusive<u64>,
        scope: Option<String>,
        series: Option<String>,
        session: &mut ClientSession,
    ) -> Result<Positions, TransactionError> {
        let mut metadata = 0;
        let roots = self.read_shard_roots(root, session, &mut metadata).await?;
        let mut selected: BTreeSet<_> = series.into_iter().collect();
        let mut hours = BTreeMap::new();
        for hour in range.start() / HOUR_MS..=range.end() / HOUR_MS {
            let current = self
                .read_shard_hours(root.generation, &roots, hour, session, &mut metadata)
                .await?;
            if let Some(scope) = &scope {
                for directory in current.values() {
                    if let Some(found) = directory.scopes.get(scope) {
                        selected.extend(found.iter().cloned());
                    }
                }
            }
            if selected.len() > MAX_ROWS {
                return Err(CaptureError::Unavailable.into());
            }
            hours.insert(hour, current);
        }

        let mut positions = BTreeMap::new();
        let mut budget = (0, 0);
        for (hour, directories) in hours {
            for series in &selected {
                let rows = self
                    .read_shard_bucket(
                        root.generation,
                        hour,
                        series,
                        &directories,
                        session,
                        &mut metadata,
                        &mut budget,
                    )
                    .await?;
                for (point, source) in rows {
                    if range.contains(&point.keys.timestamp_ms) {
                        retain(&mut positions, &source)?;
                    }
                }
            }
        }
        Ok(positions)
    }

    pub(super) async fn read_sharded_point_bucket(
        &self,
        root: &keys::ShardedGeneration,
        hour: u64,
        series: &str,
        session: &mut ClientSession,
        budget: &mut (usize, usize),
    ) -> Result<Vec<(Projection, Document)>, TransactionError> {
        let mut metadata = 0;
        let roots = self.read_shard_roots(root, session, &mut metadata).await?;
        let directories = self
            .read_shard_hours(root.generation, &roots, hour, session, &mut metadata)
            .await?;
        self.read_shard_bucket(
            root.generation,
            hour,
            series,
            &directories,
            session,
            &mut metadata,
            budget,
        )
        .await
    }

    pub(super) async fn read_shard_roots(
        &self,
        root: &keys::ShardedGeneration,
        session: &mut ClientSession,
        metadata: &mut usize,
    ) -> Result<Roots, TransactionError> {
        let ids: BTreeMap<_, _> = (0..SHARDS)
            .map(|shard| (keys::shard_id(root.generation, shard), shard))
            .collect();
        let mut cursor = self
            .shard_snapshot(ids.keys().map(String::as_str).collect(), session)
            .await?;
        let mut observed = Vec::with_capacity(usize::from(SHARDS));
        while let Some(row) = cursor.next(&mut *session).await.transpose()? {
            let id = measure(&row, metadata)?;
            let shard: keys::ShardRoot =
                open_versioned(&row, id, root.generation, keys::VERSION, None)?;
            if ids.get(id) != Some(&shard.shard) {
                return Err(CaptureError::Corrupt.into());
            }
            observed.push(shard);
        }
        let roots = root.verify_shards(observed, false)?;
        let days: BTreeSet<_> = roots
            .values()
            .flat_map(|shard| shard.days.keys())
            .map(|day| canonical_number(day).ok_or(CaptureError::Corrupt))
            .collect::<Result<_, _>>()?;
        let calendar: Calendar = self
            .authority(
                &keys::calendar_id(root.generation),
                root.generation,
                true,
                session,
            )
            .await?
            .ok_or(CaptureError::Corrupt)?;
        calendar.verify_days(&days)?;
        Ok(roots)
    }

    async fn read_shard_hours(
        &self,
        generation: u64,
        roots: &Roots,
        hour: u64,
        session: &mut ClientSession,
        metadata: &mut usize,
    ) -> Result<Hours, TransactionError> {
        let days = self
            .read_shard_days(generation, roots, hour / HOURS_PER_DAY, session, metadata)
            .await?;
        self.read_shard_hour_documents(generation, hour, &days, session, metadata)
            .await
    }

    pub(super) async fn read_shard_days(
        &self,
        generation: u64,
        roots: &Roots,
        day: u64,
        session: &mut ClientSession,
        metadata: &mut usize,
    ) -> Result<BTreeMap<u8, DayDirectory>, TransactionError> {
        let day_key = day.to_string();
        let days: BTreeMap<_, DayDirectory> = self
            .read_shard_documents(
                generation,
                roots.iter().map(|(&shard, root)| ManifestRef {
                    shard,
                    id: keys::day_id(generation, shard, day),
                    digest: root.days.get(&day_key).cloned(),
                }),
                session,
                metadata,
            )
            .await?;
        if days.values().any(|directory| {
            directory.hours.is_empty()
                || directory
                    .hours
                    .keys()
                    .any(|key| canonical_number(key).is_none_or(|hour| hour / HOURS_PER_DAY != day))
        }) {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(days)
    }

    pub(super) async fn read_shard_hour_documents(
        &self,
        generation: u64,
        hour: u64,
        days: &BTreeMap<u8, DayDirectory>,
        session: &mut ClientSession,
        metadata: &mut usize,
    ) -> Result<Hours, TransactionError> {
        let hour_key = hour.to_string();
        let hours: Hours = self
            .read_shard_documents(
                generation,
                (0..SHARDS).map(|shard| ManifestRef {
                    shard,
                    id: keys::hour_id(generation, shard, hour),
                    digest: days
                        .get(&shard)
                        .and_then(|day| day.hours.get(&hour_key))
                        .cloned(),
                }),
                session,
                metadata,
            )
            .await?;
        if hours.values().any(|directory| {
            directory.scopes.values().any(|series| {
                series.is_empty() || series.iter().any(|id| !directory.series.contains_key(id))
            })
        }) {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(hours)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn read_shard_bucket(
        &self,
        generation: u64,
        hour: u64,
        series: &str,
        directories: &Hours,
        session: &mut ClientSession,
        metadata: &mut usize,
        budget: &mut (usize, usize),
    ) -> Result<Vec<(Projection, Document)>, TransactionError> {
        let manifests: BTreeMap<_, SeriesManifest> = self
            .read_shard_documents(
                generation,
                (0..SHARDS).map(|shard| ManifestRef {
                    shard,
                    id: keys::series_id(generation, shard, hour, series),
                    digest: directories
                        .get(&shard)
                        .and_then(|d| d.series.get(series))
                        .cloned(),
                }),
                session,
                metadata,
            )
            .await?;
        let mut shards = BTreeMap::new();
        let mut leaves = BTreeMap::new();
        for (shard, manifest) in manifests {
            for (id, digest) in manifest.leaves {
                if leaves.len() == MAX_SERIES_HOUR_ROWS
                    || shards.insert(id.clone(), shard).is_some()
                {
                    return Err(CaptureError::Corrupt.into());
                }
                leaves.insert(id, digest);
            }
        }
        let rows = self
            .read_verified_bucket(
                BucketQuery {
                    generation,
                    hour,
                    series,
                    leaves,
                },
                |point| {
                    let sample = SamplePartition::for_sample(&point.keys.physical_sample_id);
                    let scope = point.scope()?;
                    Ok(point.permit_segment.is_some()
                        && shards.get(&point.source_row_id) == Some(&sample.preferred)
                        && directories
                            .get(&sample.preferred)
                            .and_then(|dir| dir.scopes.get(&scope))
                            .is_some_and(|found| found.contains(series)))
                },
                session,
                budget,
            )
            .await?;
        let expected = series_usage(&rows)?;
        let directory: Option<PermitDirectory> = self
            .authority(
                &keys::permits_id(generation, hour, series),
                generation,
                true,
                session,
            )
            .await?;
        let segments = self
            .read_permit_segments(generation, hour, series, true, session)
            .await?;
        if directory.is_some() == expected.is_empty() || segments != expected {
            return Err(CaptureError::Corrupt.into());
        }
        if let Some(directory) = directory {
            directory.verify_usage(&segments)?;
        }
        Ok(rows)
    }

    async fn read_shard_documents<T: Serialize + DeserializeOwned>(
        &self,
        generation: u64,
        refs: impl IntoIterator<Item = ManifestRef>,
        session: &mut ClientSession,
        metadata: &mut usize,
    ) -> Result<BTreeMap<u8, T>, TransactionError> {
        let mut refs: BTreeMap<_, _> = refs
            .into_iter()
            .map(|r| (r.id, (r.shard, r.digest)))
            .collect();
        let mut cursor = self
            .shard_snapshot(refs.keys().map(String::as_str).collect(), session)
            .await?;
        let mut documents = BTreeMap::new();
        while let Some(row) = cursor.next(&mut *session).await.transpose()? {
            let id = measure(&row, metadata)?;
            let (shard, expected) = refs.remove(id).ok_or(CaptureError::Corrupt)?;
            let expected = expected.ok_or(CaptureError::Corrupt)?;
            let document = open(&row, id, generation, Some(&expected))?;
            if documents.insert(shard, document).is_some() {
                return Err(CaptureError::Corrupt.into());
            }
        }
        if refs.values().any(|(_, digest)| digest.is_some()) {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(documents)
    }

    async fn shard_snapshot(
        &self,
        ids: Vec<&str>,
        session: &mut ClientSession,
    ) -> Result<mongodb::SessionCursor<Document>, TransactionError> {
        Ok(self
            .manifests
            .find(doc! {"_id": {"$in": ids}})
            .read_concern(ReadConcern::snapshot())
            .max_time(Duration::from_secs(10))
            .session(session)
            .await?)
    }
}

fn measure<'a>(row: &'a Document, metadata: &mut usize) -> Result<&'a str, CaptureError> {
    let payload = row.get_str("payload").map_err(|_| CaptureError::Corrupt)?;
    *metadata = metadata
        .checked_add(payload.len())
        .ok_or(CaptureError::Unavailable)?;
    if *metadata > MAX_MANIFEST_BYTES {
        return Err(CaptureError::Unavailable);
    }
    row.get_str("_id").map_err(|_| CaptureError::Corrupt)
}

fn series_usage(rows: &[(Projection, Document)]) -> Result<BTreeMap<u8, usize>, CaptureError> {
    let mut usage = BTreeMap::new();
    for (point, _) in rows {
        let segment = point.permit_segment.ok_or(CaptureError::Corrupt)?;
        *usage.entry(segment).or_insert(0) += 1;
    }
    Ok(usage)
}
