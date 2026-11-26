use rusqlite::Transaction;

use crate::{
    config::Budget,
    model::{Intent, Mode, Reason},
};

use super::{misses, optional, Error, Ledger, Result};

const MAX_SLOTS: i64 = 400_000;
const MAX_PENDING: i64 = 3;

pub enum Claim {
    Send,
    Skipped(Reason),
    Seen,
}

pub struct Recovery {
    pub intent: Intent,
    pub attempt: u8,
    pub previous: Option<crate::model::Observation>,
}

impl Ledger {
    pub fn claim(&self, intent: &Intent, at: i64, budget: Option<Budget>) -> Result<Claim> {
        let job = intent.job;
        let key = job.key();
        self.write(|tx, witness, fence| {
            let (since, last): (Option<i64>, Option<i64>) = tx.query_row("SELECT enabled_since,last_slot FROM jobs WHERE job=?1", [&key], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let Some(since) = since else { return Ok(Claim::Seen) };
            if last.is_some_and(|value| value >= intent.slot) || intent.slot < since || intent.slot > at {
                return Ok(Claim::Seen);
            }
            misses::record(tx, job, &intent.provenance, since, last, intent.slot, at)?;
            tx.execute("UPDATE jobs SET last_slot=?2 WHERE job=?1", (&key, intent.slot))?;
            if at >= intent.slot + job.mode.deadline() {
                return skip(tx, intent, &key, at, Reason::Missed);
            }
            let capped: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM slots ORDER BY slot LIMIT 1 OFFSET ?1)", [MAX_SLOTS - 1], |r| r.get(0))?;
            if capped {
                return Err(Error::Capacity);
            }
            if job.mode != Mode::Read && busy(tx, &intent.target)? {
                return skip(tx, intent, &key, at, Reason::Busy);
            }
            let cost = match job.mode {
                Mode::Funded => {
                    let budget = budget.ok_or(Error::Fence)?;
                    if !allows(tx, at, budget)? {
                        return skip(tx, intent, &key, at, Reason::Budget);
                    }
                    budget.max_attempt_cost
                }
                _ => 0,
            };
            let encoded = serde_json::to_string(intent)?;
            let recover_at = intent.slot + job.mode.deadline() + 3;
            tx.execute("INSERT INTO slots(job,slot,attempted_at,intent,target,funded_cost,next_recovery) VALUES(?1,?2,?3,?4,?5,?6,?7)", (&key, intent.slot, at, encoded, &intent.target, cost, recover_at))?;
            tx.execute("UPDATE jobs SET last_attempt=?2 WHERE job=?1", (&key, at))?;
            if job.mode != Mode::Read {
                Self::advance_witness(tx, witness, fence.serial)?;
            }
            Ok(Claim::Send)
        })
    }

    pub fn recovery(&self, at: i64) -> Result<Vec<Recovery>> {
        self.write(|tx, _, _| {
            let mut due = tx.prepare(
                "SELECT job,slot,intent,first_result,recovery_count FROM slots
                WHERE settled=0 AND next_recovery<=?1 AND recovery_count<3
                ORDER BY next_recovery LIMIT 12",
            )?;
            let rows = due.query_map([at], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, u8>(4)?,
                ))
            })?;
            let mut work = Vec::new();
            for row in rows {
                let (key, slot, intent, prior, count) = row?;
                let intent: Intent = serde_json::from_str(&intent)?;
                let attempt = count + 1;
                let previous = prior
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?;
                if intent.job.mode == Mode::Read || previous.is_none() {
                    super::observe::interrupted(tx, &intent, at)?;
                }
                if intent.job.mode == Mode::Read {
                    continue;
                }
                if at >= slot + 600 {
                    tx.execute(
                        "UPDATE slots SET recovery_count=3 WHERE job=?1 AND slot=?2",
                        (&key, slot),
                    )?;
                    continue;
                }
                tx.execute(
                    "UPDATE slots SET recovery_count=?3,next_recovery=?4 WHERE job=?1 AND slot=?2",
                    (&key, slot, attempt, at + i64::from(attempt) * 20),
                )?;
                work.push(Recovery {
                    intent,
                    attempt,
                    previous,
                });
            }
            Ok(work)
        })
    }
}

fn busy(tx: &Transaction<'_>, target: &str) -> Result<bool> {
    Ok(optional(tx.query_row(
        "SELECT 1 FROM slots WHERE settled=0 AND target=?1 LIMIT 1",
        [target],
        |r| r.get::<_, i64>(0),
    ))?
    .is_some()
        || tx.query_row(
            "SELECT count(*)>=?1 FROM slots AS s WHERE first_result IS NULL AND target<>''
                AND NOT EXISTS (SELECT 1 FROM recovery_placeholders AS p WHERE p.job=s.job AND p.slot=s.slot)",
            [MAX_PENDING],
            |r| r.get::<_, bool>(0),
        )?)
}

fn allows(tx: &Transaction<'_>, at: i64, budget: Budget) -> Result<bool> {
    for (window, attempts, credits) in [
        (3600, budget.hourly_attempts, budget.hourly_cost),
        (86400, budget.daily_attempts, budget.daily_cost),
    ] {
        let (count, cost): (i64, i64) = tx.query_row("SELECT count(*),COALESCE(sum(funded_cost),0) FROM slots WHERE funded_cost>0 AND attempted_at>?1", [at-window], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if count >= i64::from(attempts)
            || cost + i64::from(budget.max_attempt_cost) > i64::from(credits)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn skip(
    tx: &Transaction<'_>,
    intent: &Intent,
    key: &str,
    at: i64,
    reason: Reason,
) -> Result<Claim> {
    let result = super::observe::skipped(at, reason);
    tx.execute(
        "INSERT INTO slots(job,slot,first_result,settled,next_recovery,intent) VALUES(?1,?2,?3,1,?2,?4)",
        (key, intent.slot, serde_json::to_string(&result)?, serde_json::to_string(intent)?),
    )?;
    super::observe::first_metrics(tx, key, &result)?;
    Ok(Claim::Skipped(reason))
}
