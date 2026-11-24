use std::io::{self, Write};

use rusqlite::{Connection, Transaction};
use serde_json::json;

use crate::{
    config::Config,
    model::{Intent, Mode, Observation, Phase, Reason},
};

use super::{connect, metadata, misses, Error, Ledger, Result};

pub(super) const PLACEHOLDERS: &str = "CREATE TABLE IF NOT EXISTS recovery_placeholders (
    job TEXT NOT NULL,
    slot INTEGER NOT NULL,
    result TEXT NOT NULL,
    PRIMARY KEY (job, slot),
    FOREIGN KEY (job, slot) REFERENCES slots(job, slot) ON DELETE CASCADE
);";

pub(super) const RETENTION: i64 = 14 * 86400;

#[derive(Default)]
pub struct Metrics {
    pub jobs: Vec<JobMetrics>,
    pub counts: Vec<(String, String, String, i64)>,
    pub duration: Vec<(String, String, [i64; 8])>,
}

pub struct JobMetrics {
    pub key: String,
    pub enabled_since: Option<i64>,
    pub times: [Option<i64>; 6],
    pub active: i64,
    pub oldest: Option<i64>,
}

impl Ledger {
    pub fn first(&self, intent: &Intent, result: &Observation) -> Result<()> {
        self.write(|tx, _, _| finish(tx, intent, result))
    }

    pub fn recovered(&self, intent: &Intent, attempt: u8, result: &Observation) -> Result<()> {
        self.write(|tx, _, _| {
            let key = intent.job.key();
            let changed = tx.execute(
                "INSERT OR IGNORE INTO recovery VALUES (?1,?2,?3,?4)",
                (&key, intent.slot, attempt, serde_json::to_string(result)?),
            )?;
            if changed == 0 {
                return Ok(());
            }
            if result.settled {
                tx.execute(
                    "UPDATE slots SET settled=1 WHERE job=?1 AND slot=?2",
                    (&key, intent.slot),
                )?;
            }
            count(tx, &key, Phase::Reconcile.label(), result.reason.label(), 1)?;
            histogram(tx, &key, Phase::Reconcile.label(), result.elapsed_ms)
        })
    }

    pub fn metrics(&self) -> Result<Metrics> {
        // Serialize the snapshot with the two-file reservation commit.
        self.write(|state, _, _| read_metrics(state))
    }

    pub fn export(&self, from: i64, until: i64, config: &Config) -> Result<()> {
        let mut output = io::BufWriter::new(io::stdout().lock());
        self.export_to(from, until, config, &mut output)?;
        output.flush()?;
        Ok(())
    }

    pub fn export_to(
        &self,
        from: i64,
        until: i64,
        config: &Config,
        output: &mut impl Write,
    ) -> Result<()> {
        if from < 0 || until <= from || until - from > RETENTION {
            return Err(Error::Capacity);
        }
        let mut connection = connect(&self.state, false)?;
        let snapshot = connection.transaction()?;
        if metadata(&snapshot)?.domain != self.domain {
            return Err(Error::Fence);
        }
        write_line(
            output,
            &json!({"schema":2,"environment":config.environment,"location":config.location,
            "from":from,"until":until}),
        )?;
        let (slots, missed) = export_rows(&snapshot, output, from, until)?;
        let ranges = misses::export(&snapshot, output, from, until)?;
        write_line(
            output,
            &json!({"kind":"window_counts","slots":slots + ranges,"missed":missed + ranges}),
        )?;
        snapshot.commit()?;
        Ok(())
    }
}

fn read_metrics(state: &Connection) -> Result<Metrics> {
    let mut jobs = state.prepare("SELECT job, enabled_since, last_slot,last_attempt,last_completed,last_success,last_receipt,last_terminal,
            (SELECT count(*) FROM slots s WHERE s.job=jobs.job AND first_result IS NULL
                AND NOT EXISTS (SELECT 1 FROM recovery_placeholders p WHERE p.job=s.job AND p.slot=s.slot)),
            (SELECT min(attempted_at) FROM slots s WHERE s.job=jobs.job AND first_result IS NULL
                AND NOT EXISTS (SELECT 1 FROM recovery_placeholders p WHERE p.job=s.job AND p.slot=s.slot)) FROM jobs")?;
    let jobs = jobs
        .query_map([], |r| {
            Ok(JobMetrics {
                key: r.get(0)?,
                enabled_since: r.get(1)?,
                times: [
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ],
                active: r.get(8)?,
                oldest: r.get(9)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut counts = state.prepare("SELECT job,phase,reason,count FROM counters")?;
    let counts = counts
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut duration = state.prepare(
        "SELECT job,phase,count,sum_ms,b100,b500,b1000,b5000,b30000,b120000 FROM duration",
    )?;
    let duration = duration
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                [
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ],
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Metrics {
        jobs,
        counts,
        duration,
    })
}

pub(super) fn write_line(output: &mut impl Write, data: &serde_json::Value) -> Result<()> {
    serde_json::to_writer(&mut *output, data)?;
    writeln!(output)?;
    Ok(())
}

fn export_rows(
    conn: &Connection,
    output: &mut impl Write,
    from: i64,
    until: i64,
) -> Result<(i64, i64)> {
    let mut query = conn.prepare(
        "SELECT s.job,s.slot,s.attempted_at,s.intent,s.first_result,s.settled,p.result FROM slots s
        LEFT JOIN recovery_placeholders p ON p.job=s.job AND p.slot=s.slot
        WHERE s.slot>=?1 AND s.slot<?2 ORDER BY s.slot,s.job",
    )?;
    let mut recoveries =
        conn.prepare("SELECT result FROM recovery WHERE job=?1 AND slot=?2 ORDER BY attempt")?;
    let mut rows = query.query((from, until))?;
    let mut counts = (0, 0);
    while let Some(row) = rows.next()? {
        let job: String = row.get(0)?;
        let slot: i64 = row.get(1)?;
        let attempt: Option<i64> = row.get(2)?;
        let intent: Option<String> = row.get(3)?;
        let first: Option<String> = row.get(4)?;
        let first = first
            .map(|value| serde_json::from_str::<serde_json::Value>(&value))
            .transpose()?;
        counts.0 += 1;
        counts.1 += i64::from(
            first
                .as_ref()
                .is_some_and(|value| value["reason"] == "missed"),
        );
        let placeholder = row
            .get::<_, Option<String>>(6)?
            .map(|value| serde_json::from_str::<serde_json::Value>(&value))
            .transpose()?;
        let recovery = recoveries
            .query_map((&job, slot), |r| r.get::<_, String>(0))?
            .map(|r| serde_json::from_str::<serde_json::Value>(&r?).map_err(Error::from))
            .collect::<Result<Vec<_>>>()?;
        write_line(
            output,
            &json!({"job":job,"scheduled_at":slot,"attempted_at":attempt,
            "intent":intent.map(|v| serde_json::from_str::<serde_json::Value>(&v)).transpose()?,
            "first":first,"recovery_placeholder":placeholder,"settled":row.get::<_, bool>(5)?,"recovery":recovery}),
        )?;
    }
    Ok(counts)
}

pub(super) fn count(
    tx: &Transaction<'_>,
    key: &str,
    phase: &str,
    reason: &str,
    amount: i64,
) -> Result<()> {
    tx.execute("INSERT INTO counters VALUES(?1,?2,?3,?4) ON CONFLICT(job,phase,reason) DO UPDATE SET count=count+excluded.count", (key, phase, reason, amount))?;
    Ok(())
}

fn histogram(tx: &Transaction<'_>, key: &str, phase: &str, ms: u64) -> Result<()> {
    tx.execute("INSERT INTO duration VALUES(?1,?2,1,?3,?3<=100,?3<=500,?3<=1000,?3<=5000,?3<=30000,?3<=120000)
        ON CONFLICT(job,phase) DO UPDATE SET count=count+1,sum_ms=sum_ms+excluded.sum_ms,
        b100=b100+excluded.b100,b500=b500+excluded.b500,b1000=b1000+excluded.b1000,
        b5000=b5000+excluded.b5000,b30000=b30000+excluded.b30000,b120000=b120000+excluded.b120000", (key, phase, i64::try_from(ms).map_err(|_| Error::Capacity)?))?;
    Ok(())
}

pub(super) fn skipped(at: i64, reason: Reason) -> Observation {
    Observation {
        reason,
        phase: Phase::First,
        completed_at: at,
        elapsed_ms: 0,
        settled: true,
        evidence: Default::default(),
        reconciliation_runner_sha: None,
    }
}

pub(super) fn interrupted(tx: &Transaction<'_>, intent: &Intent, at: i64) -> Result<()> {
    let read = intent.job.mode == Mode::Read;
    let mut result = skipped(
        at,
        if read {
            Reason::Deadline
        } else {
            Reason::UnknownSend
        },
    );
    result.settled = read;
    result.phase = Phase::Provisional;
    let key = intent.job.key();
    // Recovery may start while the worker still holds an unpublished first observation.
    let changed = tx.execute(
        "INSERT INTO recovery_placeholders VALUES (?1,?2,?3)
        ON CONFLICT(job,slot) DO NOTHING",
        (&key, intent.slot, serde_json::to_string(&result)?),
    )?;
    if changed != 0 {
        count(
            tx,
            &key,
            Phase::Provisional.label(),
            result.reason.label(),
            1,
        )?;
    }
    if read {
        tx.execute(
            "UPDATE slots SET settled=1 WHERE job=?1 AND slot=?2",
            (&key, intent.slot),
        )?;
    }
    Ok(())
}

fn finish(tx: &Transaction<'_>, intent: &Intent, observation: &Observation) -> Result<()> {
    let key = intent.job.key();
    let changed = tx.execute("UPDATE slots SET first_result=?3,settled=max(settled,?4) WHERE job=?1 AND slot=?2 AND first_result IS NULL",
        (&key, intent.slot, serde_json::to_string(observation)?, observation.settled))?;
    if changed != 0 {
        first_metrics(tx, &key, observation)?;
        histogram(tx, &key, "total", observation.elapsed_ms)?;
        for (phase, duration) in [
            ("delivery", observation.evidence.delivery_elapsed_ms),
            ("verification", observation.evidence.verification_elapsed_ms),
        ] {
            if let Some(duration) = duration {
                histogram(tx, &key, phase, duration)?;
            }
        }
    }
    Ok(())
}

pub(super) fn first_metrics(tx: &Transaction<'_>, key: &str, result: &Observation) -> Result<()> {
    count(tx, key, Phase::First.label(), result.reason.label(), 1)?;
    let at = result.completed_at;
    tx.execute("UPDATE jobs SET last_completed=max(COALESCE(last_completed,0),?2),
        last_success=CASE WHEN ?3 THEN max(COALESCE(last_success,0),?2) ELSE last_success END,
        last_receipt=CASE WHEN ?4 IS NULL THEN last_receipt ELSE max(COALESCE(last_receipt,0),?4) END,
        last_terminal=CASE WHEN ?5 IS NULL THEN last_terminal ELSE max(COALESCE(last_terminal,0),?5) END WHERE job=?1",
        (key, at, result.reason == Reason::Passed, result.evidence.receipt_at, result.evidence.terminal_at))?;
    Ok(())
}
