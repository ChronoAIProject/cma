//! Verify the complete expected leaf set independently of the projection query.
use super::{model::*, store_error, ActiveGeneration, IndexedCustody, TransactionError};
use crate::store::measured_capture::{
    preview::{retain, Position, MAX_ROWS},
    sample_id, CaptureError,
};
use layer2_core::sandbox_metrics::measured_evidence::{
    batch::EvidenceScope, coverage::SUPPORT_WINDOW_MS,
};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

const MAX_READ_BYTES: usize = 16 * 1024 * 1024;
pub(super) type Positions = BTreeMap<String, Position>;

pub(super) struct BucketQuery<'a> {
    pub generation: u64,
    pub hour: u64,
    pub series: &'a str,
    pub leaves: BTreeMap<String, String>,
}

impl IndexedCustody {
    pub(in crate::store::measured_capture) async fn verify_reconciled(
        &self,
        original: &mongodb::bson::Document,
        session: &mut ClientSession,
    ) -> Result<(), CaptureError> {
        let root = self
            .active_generation(session, true)
            .await
            .map_err(store_error)?;
        let expected = Projection::from_source(ready_number(&root)?, original)?;
        let rows = self
            .read_point_bucket(&root, &expected, session, &mut (0, 0))
            .await
            .map_err(store_error)?;
        if !rows.iter().any(|(projection, source)| {
            projection.source_row_id == expected.source_row_id && source == original
        }) {
            return Err(CaptureError::Corrupt);
        }
        Ok(())
    }

    pub(in crate::store::measured_capture) async fn read_scope(
        &self,
        scope: &EvidenceScope,
        cutoff_ms: u64,
        session: &mut ClientSession,
    ) -> Result<Positions, CaptureError> {
        let from = scope.from_ms.saturating_sub(SUPPORT_WINDOW_MS);
        let to = scope.to_ms.saturating_add(SUPPORT_WINDOW_MS).min(cutoff_ms);
        self.read_range(
            from,
            to,
            Some(scope_id(&scope.owner_id, &scope.sandbox_id)?),
            None,
            session,
        )
        .await
        .map_err(store_error)
    }

    pub(in crate::store::measured_capture) async fn read_anchor(
        &self,
        anchor: &str,
        from: u64,
        to: u64,
        session: &mut ClientSession,
    ) -> Result<Positions, CaptureError> {
        let root = self
            .active_generation(session, true)
            .await
            .map_err(store_error)?;
        let generation = ready_number(&root)?;
        let original = self
            .originals
            .find_one(doc! {"_id": sample_id(anchor)})
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(|_| CaptureError::Unavailable)?
            .ok_or(CaptureError::Unavailable)?;
        let projection = Projection::from_source(generation, &original)?;
        let mut positions = self
            .read_range(
                from,
                to,
                None,
                Some(projection.keys.physical_series_id.clone()),
                session,
            )
            .await
            .map_err(store_error)?;
        // The anchor may be outside the preview. Verify its entire bucket before
        // using its physical identity, including any competing attribution.
        if !positions.contains_key(anchor) {
            let mut budget = (0, 0);
            for (point, row) in self
                .read_point_bucket(&root, &projection, session, &mut budget)
                .await
                .map_err(store_error)?
            {
                if point.keys.physical_sample_id == anchor {
                    retain(&mut positions, &row)?;
                }
            }
            if !positions.contains_key(anchor) {
                return Err(CaptureError::Corrupt);
            }
        }
        Ok(positions)
    }

    async fn read_range(
        &self,
        from: u64,
        to: u64,
        scope: Option<String>,
        series: Option<String>,
        session: &mut ClientSession,
    ) -> Result<Positions, TransactionError> {
        if from > to || to - from > 900_000 + 2 * SUPPORT_WINDOW_MS {
            return Err(CaptureError::Unavailable.into());
        }
        let generation = self.active_generation(session, true).await?;
        ready_number(&generation)?;
        let root = match generation {
            ActiveGeneration::Legacy(root) => root,
            ActiveGeneration::Sharded(root) => {
                return self
                    .read_sharded_range(&root, from..=to, scope, series, session)
                    .await;
            }
        };
        let mut hours = BTreeMap::new();
        let mut selected = BTreeSet::new();
        if let Some(series) = series {
            selected.insert(series);
        }
        for hour in from / HOUR_MS..=to / HOUR_MS {
            let directory = self.read_hour(&root, hour, session).await?;
            if let Some(series) = scope.as_ref().and_then(|scope| directory.scopes.get(scope)) {
                selected.extend(series.iter().cloned());
            }
            hours.insert(hour, directory);
        }
        // Retain the full physical series in the support window, regardless of
        // the attribution on each point. This prevents cross-transfer pairing.
        let mut positions = BTreeMap::new();
        let mut budget = (0, 0);
        for (hour, directory) in hours {
            for series in &selected {
                for (projection, row) in self
                    .read_bucket(&root, hour, series, &directory, session, &mut budget)
                    .await?
                {
                    if (from..=to).contains(&projection.keys.timestamp_ms) {
                        retain(&mut positions, &row)?;
                    }
                }
            }
        }
        Ok(positions)
    }

    async fn read_point_bucket(
        &self,
        root: &ActiveGeneration,
        point: &Projection,
        session: &mut ClientSession,
        budget: &mut (usize, usize),
    ) -> Result<Vec<(Projection, mongodb::bson::Document)>, TransactionError> {
        let hour = point.hour();
        let series = &point.keys.physical_series_id;
        match root {
            ActiveGeneration::Legacy(root) => {
                let directory = self.read_hour(root, hour, session).await?;
                self.read_bucket(root, hour, series, &directory, session, budget)
                    .await
            }
            ActiveGeneration::Sharded(root) => {
                self.read_sharded_point_bucket(root, hour, series, session, budget)
                    .await
            }
        }
    }

    pub(super) async fn read_hour(
        &self,
        root: &Generation,
        hour: u64,
        session: &mut ClientSession,
    ) -> Result<HourDirectory, TransactionError> {
        let day = hour / HOURS_PER_DAY;
        let directory: DayDirectory = self
            .manifest(
                &day_id(root.generation, day),
                root.generation,
                root.days.get(&day.to_string()).map(String::as_str),
                true,
                session,
            )
            .await?;
        let result: HourDirectory = self
            .manifest(
                &hour_id(root.generation, hour),
                root.generation,
                directory.hours.get(&hour.to_string()).map(String::as_str),
                true,
                session,
            )
            .await?;
        if result.scopes.values().any(|series| {
            series.is_empty() || series.iter().any(|key| !result.series.contains_key(key))
        }) {
            return Err(CaptureError::Corrupt.into());
        }
        Ok(result)
    }

    pub(super) async fn read_bucket(
        &self,
        root: &Generation,
        hour: u64,
        series: &str,
        directory: &HourDirectory,
        session: &mut ClientSession,
        budget: &mut (usize, usize),
    ) -> Result<Vec<(Projection, mongodb::bson::Document)>, TransactionError> {
        let expected: SeriesManifest = self
            .manifest(
                &series_id(root.generation, hour, series),
                root.generation,
                directory.series.get(series).map(String::as_str),
                true,
                session,
            )
            .await?;
        self.read_verified_bucket(
            BucketQuery {
                generation: root.generation,
                hour,
                series,
                leaves: expected.leaves,
            },
            |projection| {
                Ok(projection.permit_segment.is_none()
                    && directory
                        .scopes
                        .get(&projection.scope()?)
                        .is_some_and(|set| set.contains(series)))
            },
            session,
            budget,
        )
        .await
    }

    pub(super) async fn read_verified_bucket(
        &self,
        query: BucketQuery<'_>,
        verified_scope: impl Fn(&Projection) -> Result<bool, CaptureError>,
        session: &mut ClientSession,
        budget: &mut (usize, usize),
    ) -> Result<Vec<(Projection, mongodb::bson::Document)>, TransactionError> {
        if query.leaves.len() > MAX_SERIES_HOUR_ROWS {
            return Err(CaptureError::Corrupt.into());
        }
        let mut cursor = self
            .projections
            .find(doc! {
                "generation": signed(query.generation)?, "hour": signed(query.hour)?, "series": query.series,
            })
            .sort(doc! {"timestamp_ms":1, "source_row_id":1})
            .limit((query.leaves.len() + 1) as i64)
            .max_time(Duration::from_secs(10))
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await?;
        let mut remaining = query.leaves;
        let mut projections = Vec::new();
        while let Some(row) = cursor.next(&mut *session).await.transpose()? {
            let projection = Projection::decode(&row)?;
            if projection.generation != query.generation
                || projection.hour() != query.hour
                || projection.keys.physical_series_id != query.series
                || remaining.remove(&projection.source_row_id) != Some(projection.leaf()?)
                || !verified_scope(&projection)?
            {
                return Err(CaptureError::Corrupt.into());
            }
            budget.0 += 1;
            if budget.0 > MAX_ROWS {
                return Err(CaptureError::Unavailable.into());
            }
            projections.push(projection);
        }
        if !remaining.is_empty() {
            return Err(CaptureError::Corrupt.into());
        }
        if projections.is_empty() {
            return Ok(Vec::new());
        }
        let source_ids: Vec<_> = projections
            .iter()
            .map(|row| row.source_row_id.as_str())
            .collect();
        let mut cursor = self
            .originals
            .find(doc! {"_id": {"$in": source_ids}})
            .read_concern(ReadConcern::snapshot())
            .max_time(Duration::from_secs(10))
            .session(&mut *session)
            .await?;
        let mut originals = BTreeMap::new();
        while let Some(source) = cursor.next(&mut *session).await.transpose()? {
            budget.1 = budget
                .1
                .checked_add(
                    source
                        .get_str("payload")
                        .map_err(|_| CaptureError::Corrupt)?
                        .len(),
                )
                .ok_or(CaptureError::Unavailable)?;
            if budget.1 > MAX_READ_BYTES {
                return Err(CaptureError::Unavailable.into());
            }
            let id = source.get_str("_id").map_err(|_| CaptureError::Corrupt)?;
            if originals.insert(id.to_owned(), source).is_some() {
                return Err(CaptureError::Corrupt.into());
            }
        }
        if originals.len() != projections.len() {
            return Err(CaptureError::Corrupt.into());
        }
        projections
            .into_iter()
            .map(|projection| {
                let source = originals
                    .remove(&projection.source_row_id)
                    .ok_or(CaptureError::Corrupt)?;
                projection.verify_source(&source)?;
                Ok((projection, source))
            })
            .collect()
    }
}

fn ready_number(root: &ActiveGeneration) -> Result<u64, CaptureError> {
    let (number, phase) = match root {
        ActiveGeneration::Legacy(root) => (root.generation, root.phase),
        ActiveGeneration::Sharded(root) => (root.generation, root.phase),
    };
    if phase != Phase::Ready {
        return Err(CaptureError::Unavailable);
    }
    Ok(number)
}
