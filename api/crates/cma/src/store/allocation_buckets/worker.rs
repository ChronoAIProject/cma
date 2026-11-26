use super::*;

pub(super) const PAGE_SIZE: i64 = 4;
#[derive(Clone, Copy, Debug)]
pub(crate) enum Lane {
    Live,
    Backfill,
}
impl Lane {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Live => "allocation-buckets-v1:live",
            Self::Backfill => "allocation-buckets-v1:backfill",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Backfill => "backfill",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Progress {
    day: u64,
    cutoff: u64,
    after: Option<String>,
    pub(super) pass_complete: bool,
}
impl Progress {
    fn select(lease: &publication::Lease) -> Result<Self> {
        let today = lease.at / DAY * DAY;
        let first_day = match lease.lane {
            Lane::Live => today,
            Lane::Backfill => today.saturating_sub(DAY),
        };
        if let Some(saved) = &lease.progress {
            if !saved.day.is_multiple_of(DAY)
                || saved.day > i64::MAX as u64 / 1000 - 32 * DAY
                || !saved.cutoff.is_multiple_of(HOUR)
                || saved.cutoff < saved.day
                || saved.cutoff > saved.day + DAY
                || saved
                    .after
                    .as_ref()
                    .is_some_and(|s| s.is_empty() || s.len() > 256)
            {
                return Err(MetricsError::Unavailable);
            }
            if !saved.pass_complete
                && saved.day >= today.saturating_sub(30 * DAY)
                && saved.day <= first_day
                && (matches!(lease.lane, Lane::Backfill) || saved.day == today)
            {
                return Ok(saved.clone());
            }
        }
        Ok(Self {
            day: first_day,
            // The first UTC hour still needs boundary inputs. Its speculative
            // total is marked by the raw projector's explicit tail fields;
            // public ranges are bounded separately and use the exact input fold.
            cutoff: (first_day + DAY).min((lease.at / HOUR * HOUR).max(first_day + HOUR)),
            after: None,
            pass_complete: false,
        })
    }
    fn next(&self, lane: Lane, at: u64, last: Option<String>, more: bool) -> Self {
        if more {
            return Self {
                after: last,
                ..self.clone()
            };
        }
        let oldest = (at / DAY * DAY).saturating_sub(30 * DAY);
        match lane {
            Lane::Backfill if self.day > oldest => Self {
                day: self.day - DAY,
                cutoff: self.day,
                after: None,
                pass_complete: false,
            },
            _ => Self {
                after: None,
                pass_complete: true,
                ..self.clone()
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct Maintenance {
    pub claimed: bool,
    pub states: usize,
    pub source_rows: usize,
    pub reused: usize,
    pub written: usize,
    pub pass_complete: bool,
}

impl ReportingStore {
    pub(crate) async fn maintain_allocation_buckets(
        &self,
        enabled: bool,
        lane: Lane,
    ) -> Result<Maintenance> {
        if !enabled {
            return Ok(Maintenance::default());
        }
        self.check_circuit(super::super::usage_reports::now())?;
        let Some(lease) = self.claim_allocation_buckets(lane).await? else {
            return Ok(Maintenance::default());
        };
        let progress = Progress::select(&lease)?;
        if progress.cutoff == progress.day {
            let next = progress.next(lane, lease.at, None, false);
            self.publish_allocation_buckets(&lease, &[], &next).await?;
            return Ok(Maintenance {
                claimed: true,
                pass_complete: next.pass_complete,
                ..Default::default()
            });
        }
        let mut page_size = PAGE_SIZE;
        loop {
            match self
                .maintain_allocation_page(&lease, &progress, page_size)
                .await
            {
                // Busy is raised by the source/materialization/publication byte
                // budgets before any transaction writes. Retry a smaller page
                // under the same lease; never skip a member to make progress.
                Err(MetricsError::Busy) if page_size > 1 => page_size /= 2,
                result => return result,
            }
        }
    }

    async fn maintain_allocation_page(
        &self,
        lease: &publication::Lease,
        progress: &Progress,
        page_size: i64,
    ) -> Result<Maintenance> {
        let mut result = Maintenance {
            claimed: true,
            ..Default::default()
        };
        let mut snapshot = self.snapshot().await?;
        let mut filter = Document::new();
        if let Some(after) = &progress.after {
            filter.insert("_id", doc! {"$gt":after});
        }
        let mut cursor = self
            .collection(super::super::sandbox_usage::STATES)
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(page_size + 1)
            .batch_size((page_size + 1) as u32)
            .max_time(DB_DEADLINE)
            .session(&mut snapshot)
            .await
            .map_err(unavailable)?;
        let mut states = Vec::new();
        while let Some(document) = cursor
            .next(&mut snapshot)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            states.push(super::super::sandbox_usage::read_state(&document).map_err(unavailable)?);
        }
        let more = states.len() > page_size as usize;
        states.truncate(page_size as usize);
        let last = states.last().map(|s| s.sandbox_id.to_string());
        result.states = states.len();
        let range = Range {
            from: progress.day,
            to: progress.cutoff,
        };
        let mut missing = Vec::new();
        for state in &states {
            // Retain exact pre-creation projections too. Leaving those days
            // absent sends every long-range report back to empty ledger scans.
            let key = format!("{}:{}", state.sandbox_id, progress.day);
            let cached = self
                .collection(COLLECTION)
                .find_one(doc! {"_id":key})
                .max_time(DB_DEADLINE)
                .session(&mut snapshot)
                .await
                .map_err(unavailable)?;
            if let Some(cached) = cached {
                let bucket = DayBucket::read(&cached)?;
                if bucket.validate_source(state)?
                    && bucket.total().range.to >= range.to
                    && bucket.boundary_prepared(state)
                {
                    result.reused += 1;
                    continue;
                }
                if bucket.revision() > state.revision
                    || bucket.last_observed_at() > state.last_observed_at
                {
                    return Err(MetricsError::Unavailable);
                }
            }
            missing.push(state);
        }
        let ids: Vec<_> = missing.iter().map(|s| s.sandbox_id.as_str()).collect();
        let source_range = Range {
            from: range.from,
            to: range.from + DAY,
        };
        let events = self
            .allocation_events(&mut snapshot, &ids, source_range)
            .await?;
        result.source_rows = events.values().map(Vec::len).sum();
        let buckets = missing
            .into_iter()
            .map(|state| {
                materialize::build_covered(
                    state,
                    events
                        .get(state.sandbox_id.as_str())
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    range,
                    source_range.to,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        result.written = buckets.len();
        let next = progress.next(lease.lane, lease.at, last, more);
        result.pass_complete = next.pass_complete;
        self.publish_allocation_buckets(lease, &buckets, &next)
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(progress: Progress) -> publication::Lease {
        publication::Lease {
            lane: Lane::Live,
            holder: "test".into(),
            generation: 1,
            at: 100 * DAY + 6 * HOUR,
            progress: Some(progress),
        }
    }

    #[test]
    fn live_resume_keeps_cutoff_until_finished_and_prioritizes_the_new_day() {
        let mut lease = lease(Progress {
            day: 100 * DAY,
            cutoff: 100 * DAY + 5 * HOUR,
            after: Some("sandbox".into()),
            pass_complete: false,
        });
        let resumed = Progress::select(&lease).unwrap();
        assert_eq!(resumed.cutoff, 100 * DAY + 5 * HOUR);
        assert_eq!(resumed.after.as_deref(), Some("sandbox"));
        lease.progress.as_mut().unwrap().pass_complete = true;
        let refreshed = Progress::select(&lease).unwrap();
        assert_eq!(refreshed.cutoff, 100 * DAY + 6 * HOUR);
        assert!(refreshed.after.is_none());
        lease.progress.as_mut().unwrap().pass_complete = false;
        lease.at += DAY;
        let tomorrow = Progress::select(&lease).unwrap();
        assert_eq!(tomorrow.day, 101 * DAY);
        assert!(tomorrow.after.is_none());
    }

    #[test]
    fn malformed_durable_progress_refuses_before_scanning_or_overflow() {
        let mut lease = lease(Progress {
            day: u64::MAX / DAY * DAY,
            cutoff: u64::MAX,
            after: None,
            pass_complete: false,
        });
        assert!(Progress::select(&lease).is_err());
        lease.progress = Some(Progress {
            day: 100 * DAY,
            cutoff: 100 * DAY + 1,
            after: None,
            pass_complete: false,
        });
        assert!(Progress::select(&lease).is_err());
    }
}
