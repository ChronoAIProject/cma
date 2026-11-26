use crate::sandbox_usage::{UsageError, UsageEvent, UsageTotals};
use serde::{Deserialize, Serialize};

pub const MAX_FACTS: usize = 64;
const MINUTE: u64 = 60;
const MAX_SECONDS: u64 = i64::MAX as u64 / 1_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinuteFact {
    pub part: u64,
    pub bucket_start: u64,
    pub started_at: u64,
    pub ended_at: u64,
    pub quantity: UsageTotals,
    pub retained_home_seconds: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Projection {
    pub facts: Vec<MinuteFact>,
    pub next_second: u64,
    pub complete: bool,
}

/// A cursor is either the source start, a minute boundary, or the source end.
/// Empty source transitions complete without manufacturing an interval.
pub fn project(event: &UsageEvent, cursor: u64) -> Result<Projection, UsageError> {
    if event.started_at > event.ended_at
        || event.ended_at > MAX_SECONDS
        || !(event.started_at..=event.ended_at).contains(&cursor)
        || (cursor != event.started_at
            && cursor != event.ended_at
            && !cursor.is_multiple_of(MINUTE))
    {
        return Err(UsageError::InvalidTime);
    }
    if event.quantity != event.allocation.usage(event.ended_at - event.started_at)?
        || (!event.allocation.running
            && (event.allocation.cpu_millis != 0 || event.allocation.memory_mib != 0))
    {
        return Err(UsageError::Conflict);
    }
    let mut next = cursor;
    let mut facts = Vec::new();
    while next < event.ended_at && facts.len() < MAX_FACTS {
        let bucket = next / MINUTE * MINUTE;
        let end = (bucket + MINUTE).min(event.ended_at);
        let seconds = end - next;
        facts.push(MinuteFact {
            part: bucket / MINUTE - event.started_at / MINUTE,
            bucket_start: bucket,
            started_at: next,
            ended_at: end,
            quantity: event.allocation.usage(seconds)?,
            retained_home_seconds: if event.allocation.disk_gib > 0 {
                seconds
            } else {
                0
            },
        });
        next = end;
    }
    Ok(Projection {
        facts,
        next_second: next,
        complete: next == event.ended_at,
    })
}
