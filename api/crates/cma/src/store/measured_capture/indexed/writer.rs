//! Maintain the authenticated discovery and exact leaf set in the source transaction.
use super::{model::*, IndexedCustody};
use crate::store::measured_capture::{CaptureError, Document, TransactionError};
use mongodb::{bson::doc, ClientSession};

impl IndexedCustody {
    pub(in crate::store::measured_capture) async fn append(
        &self,
        source: &Document,
        inserted: bool,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let mut root = self.root(session, false).await?;
        if root.phase != Phase::Ready {
            return Err(CaptureError::Unavailable.into());
        }
        let projection = Projection::from_source(root.generation, source)?;
        if !inserted {
            return self.verify_existing(&root, &projection, session).await;
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

    async fn verify_existing(
        &self,
        root: &Generation,
        projection: &Projection,
        session: &mut ClientSession,
    ) -> Result<(), TransactionError> {
        let row = self
            .projections
            .find_one(doc! {"_id": projection.id()})
            .session(&mut *session)
            .await?
            .ok_or(CaptureError::Corrupt)?;
        if Projection::decode(&row)? != *projection {
            return Err(CaptureError::Corrupt.into());
        }
        let (day, hour, series) = self.path(root, projection, session).await?;
        if !day.hours.contains_key(&projection.hour().to_string())
            || !hour
                .scopes
                .get(&projection.scope()?)
                .is_some_and(|series| series.contains(&projection.keys.physical_series_id))
            || series.leaves.get(&projection.source_row_id) != Some(&projection.leaf()?)
        {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(())
    }

    async fn path(
        &self,
        root: &Generation,
        projection: &Projection,
        session: &mut ClientSession,
    ) -> Result<(DayDirectory, HourDirectory, SeriesManifest), TransactionError> {
        let generation = root.generation;
        let day: DayDirectory = self
            .manifest(
                &day_id(generation, projection.day()),
                generation,
                root.days
                    .get(&projection.day().to_string())
                    .map(String::as_str),
                false,
                session,
            )
            .await?;
        let hour: HourDirectory = self
            .manifest(
                &hour_id(generation, projection.hour()),
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
                &series_id(
                    generation,
                    projection.hour(),
                    &projection.keys.physical_series_id,
                ),
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
        let generation = root.generation;
        let day_key = projection.day().to_string();
        let hour_key = projection.hour().to_string();
        let series_key = &projection.keys.physical_series_id;
        let (mut day, mut hour, mut series) = self.path(root, projection, session).await?;
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
                &series_id(generation, projection.hour(), series_key),
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
                &hour_id(generation, projection.hour()),
                generation,
                day.hours.get(&hour_key).map(String::as_str),
                &hour,
                session,
            )
            .await?;
        day.hours.insert(hour_key, sealed_hour);
        let sealed_day = self
            .replace_manifest(
                &day_id(generation, projection.day()),
                generation,
                root.days.get(&day_key).map(String::as_str),
                &day,
                session,
            )
            .await?;
        root.days.insert(day_key, sealed_day);
        self.projections
            .insert_one(projection.document()?)
            .session(session)
            .await?;
        root.rows = root.rows.checked_add(1).ok_or(CaptureError::Unavailable)?;
        Ok(())
    }
}
