use std::io::Write;

use rusqlite::{Connection, Transaction};
use serde_json::json;

use crate::model::{Job, Provenance, Reason};

use super::{
    observe::{count, write_line},
    Result,
};

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS missed_ranges (
    job TEXT NOT NULL,
    first_slot INTEGER NOT NULL,
    last_slot INTEGER NOT NULL,
    slot_interval INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL,
    configuration TEXT NOT NULL,
    candidate TEXT NOT NULL,
    runner_sha TEXT NOT NULL,
    PRIMARY KEY (job, first_slot)
);
CREATE INDEX IF NOT EXISTS missed_range_end ON missed_ranges(last_slot);";

pub(super) fn record(
    tx: &Transaction<'_>,
    job: Job,
    provenance: &Provenance,
    since: i64,
    last: Option<i64>,
    before: i64,
    observed: i64,
) -> Result<()> {
    let interval = job.mode.interval();
    let active_start = job.slot_at(since - 1) + interval;
    let first = last.map_or(active_start, |slot| (slot + interval).max(active_start));
    let end = job.slot_at(before - 1);
    if first <= end {
        let key = job.key();
        // Persist the original UTC slots compactly even after extended downtime.
        tx.execute(
            "INSERT INTO missed_ranges VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            (
                &key,
                first,
                end,
                interval,
                observed,
                &provenance.configuration,
                &provenance.candidate,
                &provenance.runner_sha,
            ),
        )?;
        count(
            tx,
            &key,
            "first",
            Reason::Missed.label(),
            1 + (end - first) / interval,
        )?;
    }
    Ok(())
}

pub(super) fn export(
    connection: &Connection,
    output: &mut impl Write,
    from: i64,
    until: i64,
) -> Result<i64> {
    let mut ranges = connection.prepare("SELECT job,first_slot,last_slot,slot_interval,recorded_at,configuration,candidate,runner_sha
        FROM missed_ranges WHERE last_slot>=?1 AND first_slot<?2 ORDER BY first_slot,job")?;
    let mut rows = ranges.query((from, until))?;
    let mut total = 0;
    while let Some(row) = rows.next()? {
        let start: i64 = row.get(1)?;
        let end: i64 = row.get(2)?;
        let interval: i64 = row.get(3)?;
        let first = start + (from - start).max(0).div_euclid(interval) * interval;
        let first = if first < from {
            first + interval
        } else {
            first
        };
        let last = start + (end.min(until - 1) - start).div_euclid(interval) * interval;
        if first > last {
            continue;
        }
        let count = 1 + (last - first) / interval;
        total += count;
        write_line(
            output,
            &json!({"kind":"missed_range","job":row.get::<_, String>(0)?,
            "first_scheduled_at":first,"last_scheduled_at":last,"interval_seconds":interval,"count":count,
            "recorded_at":row.get::<_, i64>(4)?,"configuration":row.get::<_, String>(5)?,
            "candidate":row.get::<_, String>(6)?,"recording_runner_sha":row.get::<_, String>(7)?}),
        )?;
    }
    Ok(total)
}
