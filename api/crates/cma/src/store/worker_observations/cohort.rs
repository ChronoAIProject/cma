//! Read-only fixed admission cohorts, bounded independently of scrape traffic.
//! Counts and classifications share a Mongo snapshot, not six separate views.
use crate::telemetry::task_cohort::Cache;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::{Hint, ReadConcern},
    ClientSession, Database,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod fold;
use fold::Summary;
#[cfg(test)]
mod tests;

pub(in crate::store) const INDEX: &str = "agent_command_admission_cohort";
const MAX_ROWS: i64 = 10_000;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const WINDOWS: [&str; 4] = [
    "current_day",
    "previous_hour",
    "previous_day",
    "previous_7_days",
];
const IO: Duration = Duration::from_secs(1);

pub(super) struct Observer {
    database: Database,
    cache: Cache,
}
impl Observer {
    pub(super) fn new(database: Database, cache: Cache) -> Self {
        cache.initialize(&WINDOWS);
        Self { database, cache }
    }
    pub(super) async fn sample(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        for window in windows(now) {
            let result = tokio::time::timeout(Duration::from_secs(3), self.read(window)).await;
            self.cache
                .update(window.name, result.unwrap_or(Err("timeout")));
        }
    }
    async fn read(&self, window: Window) -> Result<String, &'static str> {
        super::super::migrations::require_command_cohort_index(&self.database)
            .await
            .map_err(|_| "database")?;
        let mut session = self
            .database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| "database")?;
        let denominator = self.denominator(window, &mut session).await?;
        let mut summary = Summary::new(window, denominator);
        let mut rows = self
            .database
            .collection::<Document>("agent_commands")
            .aggregate([
                doc! {"$match":window.filter()},
                doc! {"$limit":MAX_ROWS + 1},
                doc! {"$project":fold::projection()},
            ])
            .hint(Hint::Name(INDEX.into()))
            .read_concern(ReadConcern::snapshot())
            .max_time(IO)
            .batch_size(100)
            .allow_disk_use(false)
            .session(&mut session)
            .await
            .map_err(|_| "database")?;
        let mut used = 0;
        while let Some(row) = rows
            .stream(&mut session)
            .try_next()
            .await
            .map_err(|_| "database")?
        {
            used += mongodb::bson::serialize_to_vec(&row)
                .map_err(|_| "invalid_snapshot")?
                .len();
            if used > MAX_BYTES || summary.rows >= MAX_ROWS as u64 {
                return Err("budget");
            }
            summary.observe(row)?;
        }
        let snapshot = session.snapshot_time().ok_or("invalid_snapshot")?;
        summary.finish(snapshot)
    }
    async fn denominator(
        &self,
        window: Window,
        session: &mut ClientSession,
    ) -> Result<fold::Denominator, &'static str> {
        // Independent count by immutable command kind, not a sum of status classes.
        let mut rows = self.database.collection::<Document>("agent_commands")
            .aggregate([
                doc! {"$match":window.filter()},
                doc! {"$limit":MAX_ROWS + 1},
                doc! {"$group":{"_id":null,"records":{"$sum":1},
                    "tasks":{"$sum":{"$cond":[{"$eq":["$record.receipt.envelope.command.type","message"]},1,0]}}}},
            ])
            .hint(Hint::Name(INDEX.into())).read_concern(ReadConcern::snapshot())
            .max_time(IO).allow_disk_use(false).session(&mut *session).await.map_err(|_| "database")?;
        let Some(row) = rows
            .stream(session)
            .try_next()
            .await
            .map_err(|_| "database")?
        else {
            return Ok(fold::Denominator {
                records: 0,
                tasks: 0,
            });
        };
        let value = |key| {
            row.get_i64(key)
                .or_else(|_| row.get_i32(key).map(i64::from))
                .map_err(|_| "invalid_snapshot")
        };
        let records = value("records")?;
        let tasks = value("tasks")?;
        if records > MAX_ROWS {
            return Err("budget");
        }
        if records < 0 || tasks < 0 || tasks > records {
            return Err("invalid_snapshot");
        }
        Ok(fold::Denominator {
            records: records as u64,
            tasks: tasks as u64,
        })
    }
}

#[derive(Clone, Copy)]
struct Window {
    name: &'static str,
    from: u64,
    to: u64,
    observed: u64,
}
impl Window {
    fn filter(self) -> Document {
        let at = |seconds| layer2_core::rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds));
        doc! {"record.receipt.admitted_at":{"$gte":at(self.from),"$lt":at(self.to)}}
    }
}
fn windows(now: u64) -> [Window; 4] {
    let day = now / 86_400 * 86_400;
    let hour = now / 3_600 * 3_600;
    [
        Window {
            name: WINDOWS[0],
            from: day,
            to: now,
            observed: now,
        },
        Window {
            name: WINDOWS[1],
            from: hour.saturating_sub(3_600),
            to: hour,
            observed: now,
        },
        Window {
            name: WINDOWS[2],
            from: day.saturating_sub(86_400),
            to: day,
            observed: now,
        },
        Window {
            name: WINDOWS[3],
            from: day.saturating_sub(604_800),
            to: day,
            observed: now,
        },
    ]
}
