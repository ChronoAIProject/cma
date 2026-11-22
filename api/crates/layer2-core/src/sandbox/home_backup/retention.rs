//! Fixed retention policy for the ready history of one home.
use std::collections::BTreeSet;

use super::{model::timestamp, HomeBackupError, HomeBackupRetention, HomeBackupRetentionClass};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;
const SGT_OFFSET: u64 = 8 * HOUR;
const MAX_RETENTION: u64 = 30 * DAY;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyPoint {
    pub backup_id: String,
    pub ready_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionDecision {
    pub backup_id: String,
    pub retention: HomeBackupRetention,
}

/// Supply every ready point in the relevant hours/days, including deleted
/// records: deletion must not promote a later point into the first daily one.
/// Results are ordered by ready time, then identity, independent of input order.
/// The latest short point waits for a strictly newer ready time, subject to the
/// same immutable thirty-day limit as every other point. No policy is configurable.
pub fn retain_ready_points(
    points: &[ReadyPoint],
) -> Result<Vec<RetentionDecision>, HomeBackupError> {
    let mut identities = BTreeSet::new();
    let mut ordered = Vec::with_capacity(points.len());
    for point in points {
        if point.backup_id.is_empty()
            || point.backup_id.trim() != point.backup_id
            || point.backup_id.chars().any(char::is_control)
            || !identities.insert(&point.backup_id)
        {
            return Err(HomeBackupError::Invalid(
                "invalid or duplicate ready identity",
            ));
        }
        ordered.push((timestamp(&point.ready_at)?, point.backup_id.as_str()));
    }
    ordered.sort_unstable();
    let mut days = BTreeSet::new();
    let mut hours = BTreeSet::new();
    let mut newer = None;
    let mut successors = vec![None; ordered.len()];
    for index in (0..ordered.len()).rev() {
        if index + 1 < ordered.len() && ordered[index + 1].0 > ordered[index].0 {
            newer = Some(ordered[index + 1].0);
        }
        successors[index] = newer;
    }
    ordered
        .iter()
        .enumerate()
        .map(|(index, &(ready, id))| {
            let cap = add(ready, MAX_RETENTION)?;
            let first_day = days.insert(add(ready, SGT_OFFSET)? / DAY);
            let first_hour = hours.insert(ready / HOUR);
            let (class, expires) = if first_day {
                (HomeBackupRetentionClass::Daily, cap)
            } else if first_hour {
                (HomeBackupRetentionClass::Hourly, add(ready, 7 * DAY)?)
            } else {
                let expires = successors[index]
                    .map(|at| add(at, 2 * HOUR))
                    .transpose()?
                    .unwrap_or(cap)
                    .min(cap);
                (HomeBackupRetentionClass::Short, expires)
            };
            let at = std::time::UNIX_EPOCH
                .checked_add(std::time::Duration::from_secs(expires))
                .ok_or(HomeBackupError::Exhausted)?;
            let expires_at = crate::rfc3339_at(at);
            timestamp(&expires_at)?;
            Ok(RetentionDecision {
                backup_id: id.into(),
                retention: HomeBackupRetention { class, expires_at },
            })
        })
        .collect()
}

fn add(at: u64, duration: u64) -> Result<u64, HomeBackupError> {
    at.checked_add(duration).ok_or(HomeBackupError::Exhausted)
}
