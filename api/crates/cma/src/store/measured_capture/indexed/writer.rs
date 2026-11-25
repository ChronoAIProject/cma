//! Maintain the authenticated discovery and exact leaf set in the source transaction.
use super::{model::*, sharded_generation, ActiveGeneration, IndexedCustody};
use crate::store::measured_capture::{CaptureError, Document, TransactionError};
use mongodb::{bson::doc, ClientSession};
use std::collections::BTreeMap;

struct ProjectionPath {
    day: String,
    hour: String,
    series: String,
}

impl ProjectionPath {
    fn for_projection(projection: &Projection, shard: Option<u8>) -> Self {
        let generation = projection.generation;
        let day = projection.day();
        let hour = projection.hour();
        let series = &projection.keys.physical_series_id;
        if let Some(shard) = shard {
            Self {
                day: sharded_generation::day_id(generation, shard, day),
                hour: sharded_generation::hour_id(generation, shard, hour),
                series: sharded_generation::series_id(generation, shard, hour, series),
            }
        } else {
            Self {
                day: day_id(generation, day),
                hour: hour_id(generation, hour),
                series: series_id(generation, hour, series),
            }
        }
    }
}

impl IndexedCustody {
    pub(in crate::store::measured_capture) async fn append(
        &self,
        source: &Document,
        inserted: bool,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let mut root = match self.active_generation(session, false).await? {
            ActiveGeneration::Legacy(root) => root,
            ActiveGeneration::Sharded(root) => {
                return self.append_sharded(&root, source, inserted, session).await
            }
        };
        if root.phase != Phase::Ready {
            return Err(CaptureError::Unavailable.into());
        }
        let projection = Projection::from_source(root.generation, source)?;
        if !inserted {
            return self
                .verify_existing_projection(&root.days, &projection, None, session)
                .await
                .map(|_| ());
        }
        let previous = digest(&root)?;
        self.insert_projection(&mut root, &projection, session)
            .await?;
        self.publish_root(&mut root, &previous, session).await
    }

    pub(super) async fn publish_root(
        &self,
        root: &mut Generation,
        previous: &str,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        root.revision = root
            .revision
            .checked_add(1)
            .ok_or(CaptureError::Unavailable)?;
        root.validate()?;
        self.replace_manifest(ROOT_ID, root.generation, Some(previous), root, session)
            .await?;
        Ok(())
    }

    pub(super) async fn verify_existing_projection(
        &self,
        days: &BTreeMap<String, String>,
        projection: &Projection,
        shard: Option<u8>,
        session: &mut ClientSession,
    ) -> Result<Projection, TransactionError> {
        let row = self
            .projections
            .find_one(doc! {"_id": projection.id()})
            .session(&mut *session)
            .await?
            .ok_or(CaptureError::Corrupt)?;
        let stored = Projection::decode(&row)?;
        let mut expected = projection.clone();
        if stored.permit_segment.is_some() != shard.is_some() {
            return Err(CaptureError::Corrupt.into());
        }
        expected.permit_segment = stored.permit_segment;
        if expected != stored {
            return Err(CaptureError::Corrupt.into());
        }
        let ids = ProjectionPath::for_projection(&stored, shard);
        let (day, hour, series) = self.path(days, &stored, &ids, session).await?;
        if !day.hours.contains_key(&projection.hour().to_string())
            || !hour
                .scopes
                .get(&projection.scope()?)
                .is_some_and(|series| series.contains(&projection.keys.physical_series_id))
            || series.leaves.get(&stored.source_row_id) != Some(&stored.leaf()?)
        {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(stored)
    }

    async fn path(
        &self,
        days: &BTreeMap<String, String>,
        projection: &Projection,
        ids: &ProjectionPath,
        session: &mut ClientSession,
    ) -> Result<(DayDirectory, HourDirectory, SeriesManifest), TransactionError> {
        let generation = projection.generation;
        let day: DayDirectory = self
            .manifest(
                &ids.day,
                generation,
                days.get(&projection.day().to_string()).map(String::as_str),
                false,
                session,
            )
            .await?;
        let hour: HourDirectory = self
            .manifest(
                &ids.hour,
                generation,
                day.hours
                    .get(&projection.hour().to_string())
                    .map(String::as_str),
                false,
                session,
            )
            .await?;
        let series: SeriesManifest = self
            .manifest(
                &ids.series,
                generation,
                hour.series
                    .get(&projection.keys.physical_series_id)
                    .map(String::as_str),
                false,
                session,
            )
            .await?;
        Ok((day, hour, series))
    }

    pub(super) async fn insert_projection(
        &self,
        root: &mut Generation,
        projection: &Projection,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        self.insert_projection_at(&mut root.days, projection, None, session)
            .await?;
        root.rows = root.rows.checked_add(1).ok_or(CaptureError::Unavailable)?;
        Ok(())
    }

    pub(super) async fn insert_projection_at(
        &self,
        days: &mut BTreeMap<String, String>,
        projection: &Projection,
        shard: Option<u8>,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let generation = projection.generation;
        let ids = ProjectionPath::for_projection(projection, shard);
        let day_key = projection.day().to_string();
        let hour_key = projection.hour().to_string();
        let series_key = &projection.keys.physical_series_id;
        let (mut day, mut hour, mut series) = self.path(days, projection, &ids, session).await?;
        if series.leaves.len() >= MAX_SERIES_HOUR_ROWS
            || series
                .leaves
                .insert(projection.source_row_id.clone(), projection.leaf()?)
                .is_some()
        {
            return Err(CaptureError::Unavailable.into());
        }
        let sealed_series = self
            .replace_manifest(
                &ids.series,
                generation,
                hour.series.get(series_key).map(String::as_str),
                &series,
                session,
            )
            .await?;
        hour.series.insert(series_key.clone(), sealed_series);
        hour.scopes
            .entry(projection.scope()?)
            .or_default()
            .insert(series_key.clone());
        let sealed_hour = self
            .replace_manifest(
                &ids.hour,
                generation,
                day.hours.get(&hour_key).map(String::as_str),
                &hour,
                session,
            )
            .await?;
        day.hours.insert(hour_key, sealed_hour);
        let sealed_day = self
            .replace_manifest(
                &ids.day,
                generation,
                days.get(&day_key).map(String::as_str),
                &day,
                session,
            )
            .await?;
        days.insert(day_key, sealed_day);
        self.projections
            .insert_one(projection.document()?)
            .session(session)
            .await?;
        Ok(())
    }
}
