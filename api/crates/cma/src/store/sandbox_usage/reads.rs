use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::{Page, PageRequest, MAX_PAGE_SIZE};
use layer2_core::sandbox_usage::UsageTotals;
use layer2_core::sandbox_usage_cursor::UsageSnapshot;
use layer2_core::sandbox_usage_service::{SandboxUsageStore, UsageSummary};
use layer2_core::{SandboxId, SubjectId};
use mongodb::{
    bson::{Bson, Timestamp},
    error::ErrorKind,
    Collection,
};
use std::time::Duration;

#[async_trait::async_trait]
impl SandboxUsageStore for MongoSandboxStore {
    async fn usage(&self, id: &SandboxId) -> Result<Option<SandboxUsage>, SandboxStoreError> {
        self.sandbox_usage
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(unavailable)?
            .as_ref()
            .map(read_state)
            .transpose()
    }

    async fn usage_page(
        &self,
        owner: Option<&SubjectId>,
        page: &PageRequest,
    ) -> Result<Page<SandboxUsage>, SandboxStoreError> {
        let (filter, sort) = sandbox_page_query(owner, page)?;
        live_page(
            &self.sandbox_usage,
            filter,
            sort,
            page.limit,
            read_state,
            |state| state.sandbox_id.to_string(),
        )
        .await
    }

    async fn usage_snapshot_page(
        &self,
        owner: Option<&SubjectId>,
        page: &PageRequest,
        snapshot: Option<UsageSnapshot>,
    ) -> Result<(Page<SandboxUsage>, UsageSnapshot), SandboxStoreError> {
        let (filter, sort) = sandbox_page_query(owner, page)?;
        let (rows, observed) =
            snapshot_rows(&self.sandbox_usage, filter, sort, page.limit, snapshot).await?;
        let result = project_page(rows, page.limit, read_state, |state| {
            state.sandbox_id.to_string()
        })?;
        Ok((result, observed))
    }

    async fn usage_summary(
        &self,
        owner: Option<&SubjectId>,
        as_of: u64,
    ) -> Result<UsageSummary, SandboxStoreError> {
        let as_of = signed(as_of, "usage as_of").map_err(SandboxStoreError::Corrupt)?;
        let delta = doc! { "$max": [0, { "$subtract": [as_of, "$record.last_observed_at"] }] };
        let sum = |total: &str, resource: Bson| {
            doc! { "$sum": { "$add": [
                format!("$record.totals.{total}"), { "$multiply": [resource, delta.clone()] }
            ] } }
        };
        let pipeline = vec![
            doc! { "$match": owner_filter(owner) },
            doc! { "$set": {
                "record.allocation.running": { "$cond": [
                    { "$in": ["$record.status", ["failed", "retired"]] }, false, "$record.allocation.running"
                ] },
                "record.allocation.cpu_millis": { "$cond": [
                    { "$in": ["$record.status", ["failed", "retired"]] }, 0, "$record.allocation.cpu_millis"
                ] },
                "record.allocation.memory_mib": { "$cond": [
                    { "$in": ["$record.status", ["failed", "retired"]] }, 0, "$record.allocation.memory_mib"
                ] },
            } },
            doc! { "$group": {
                "_id": Bson::Null, "sandbox_count": { "$sum": 1 },
                "running_count": { "$sum": { "$cond": ["$record.allocation.running", 1, 0] } },
                "incomplete": { "$sum": { "$cond": ["$record.complete_lifecycle", 0, 1] } },
                "tracking_started_at": { "$min": "$record.tracking_started_at" },
                "uptime_seconds": sum("uptime_seconds", doc! { "$cond": ["$record.allocation.running", 1, 0] }.into()),
                "cpu_millis_seconds": sum("cpu_millis_seconds", "$record.allocation.cpu_millis".into()),
                "memory_mib_seconds": sum("memory_mib_seconds", "$record.allocation.memory_mib".into()),
                "disk_gib_seconds": sum("disk_gib_seconds", "$record.allocation.disk_gib".into()),
            } },
        ];
        let mut cursor = self
            .sandbox_usage
            .aggregate(pipeline)
            .max_time(std::time::Duration::from_secs(10))
            .await
            .map_err(unavailable)?;
        let Some(row) = cursor.try_next().await.map_err(unavailable)? else {
            return Ok(UsageSummary {
                sandbox_count: 0,
                running_count: 0,
                incomplete_lifecycle_count: 0,
                tracking_started_at: None,
                totals: UsageTotals::default(),
            });
        };
        Ok(UsageSummary {
            sandbox_count: number(&row, "sandbox_count")?,
            running_count: number(&row, "running_count")?,
            incomplete_lifecycle_count: number(&row, "incomplete")?,
            tracking_started_at: Some(number(&row, "tracking_started_at")?),
            totals: UsageTotals {
                uptime_seconds: number(&row, "uptime_seconds")?,
                cpu_millis_seconds: number(&row, "cpu_millis_seconds")?,
                memory_mib_seconds: number(&row, "memory_mib_seconds")?,
                disk_gib_seconds: number(&row, "disk_gib_seconds")?,
            },
        })
    }

    async fn usage_events(
        &self,
        id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<UsageEvent>, SandboxStoreError> {
        let (filter, sort) = event_page_query(id, page)?;
        live_page(
            &self.sandbox_usage_events,
            filter,
            sort,
            page.limit,
            read_event,
            |event| event.revision.to_string(),
        )
        .await
    }

    async fn usage_snapshot_events(
        &self,
        id: &SandboxId,
        page: &PageRequest,
        snapshot: Option<UsageSnapshot>,
    ) -> Result<(Page<UsageEvent>, UsageSnapshot), SandboxStoreError> {
        let (filter, sort) = event_page_query(id, page)?;
        let (rows, observed) = snapshot_rows(
            &self.sandbox_usage_events,
            filter,
            sort,
            page.limit,
            snapshot,
        )
        .await?;
        let result = project_page(rows, page.limit, read_event, |event| {
            event.revision.to_string()
        })?;
        Ok((result, observed))
    }
}

fn sandbox_page_query(
    owner: Option<&SubjectId>,
    page: &PageRequest,
) -> Result<(Document, Document), SandboxStoreError> {
    validate_page(page)?;
    let mut filter = owner_filter(owner);
    if let Some(cursor) = &page.cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }
    Ok((filter, doc! { "_id": 1 }))
}

fn event_page_query(
    id: &SandboxId,
    page: &PageRequest,
) -> Result<(Document, Document), SandboxStoreError> {
    validate_page(page)?;
    let mut filter = doc! { "sandbox_id": id.as_str() };
    if let Some(cursor) = &page.cursor {
        let revision = cursor
            .parse::<i64>()
            .ok()
            .filter(|v| *v > 0)
            .ok_or(SandboxStoreError::InvalidPage)?;
        filter.insert("revision", doc! { "$gt": revision });
    }
    Ok((filter, doc! { "revision": 1 }))
}

async fn live_page<T>(
    source: &Collection<Document>,
    filter: Document,
    sort: Document,
    limit: u16,
    read: impl Fn(&Document) -> Result<T, SandboxStoreError>,
    key: impl Fn(&T) -> String,
) -> Result<Page<T>, SandboxStoreError> {
    let rows = source
        .find(filter)
        .sort(sort)
        .limit(i64::from(limit) + 1)
        .await
        .map_err(unavailable)?
        .try_collect()
        .await
        .map_err(unavailable)?;
    project_page(rows, limit, read, key)
}

async fn snapshot_rows(
    source: &Collection<Document>,
    filter: Document,
    sort: Document,
    limit: u16,
    snapshot: Option<UsageSnapshot>,
) -> Result<(Vec<Document>, UsageSnapshot), SandboxStoreError> {
    let collection = crate::store::snapshot_collection::collection(source, true);
    let expected = snapshot.map(|snapshot| Timestamp {
        time: snapshot.time,
        increment: snapshot.increment,
    });
    let start = source.client().start_session().snapshot(true);
    let mut session = match expected {
        Some(timestamp) => start.snapshot_time(timestamp).await,
        None => start.await,
    }
    .map_err(unavailable)?;
    let map_error = |error: mongodb::error::Error| {
        // SnapshotTooOld is definitive for the timestamp in a continuation.
        if expected.is_some()
            && matches!(error.kind.as_ref(), ErrorKind::Command(command) if command.code == 286)
        {
            SandboxStoreError::InvalidPage
        } else {
            unavailable(error)
        }
    };
    let mut cursor = collection
        .find(filter)
        .sort(sort)
        .limit(i64::from(limit) + 1)
        .max_time(Duration::from_secs(10))
        .session(&mut session)
        .await
        .map_err(map_error)?;
    let rows: Vec<Document> = cursor
        .stream(&mut session)
        .try_collect()
        .await
        .map_err(map_error)?;
    // Account erasure can remove usage while the historical view still exists.
    // Verify current ownership and presence for the entire page and its lookahead.
    if !rows.is_empty() {
        let identities = rows
            .iter()
            .map(|row| {
                let invalid = |_| SandboxStoreError::Corrupt("invalid usage row identity".into());
                Ok(doc! { "_id": row.get_str("_id").map_err(invalid)?,
                "owner": row.get_str("owner").map_err(invalid)? })
            })
            .collect::<Result<Vec<_>, SandboxStoreError>>()?;
        let current_count = source
            .count_documents(doc! { "$or": identities })
            .max_time(Duration::from_secs(10))
            .await
            .map_err(unavailable)?;
        if current_count != rows.len() as u64 {
            return Err(if expected.is_some() {
                SandboxStoreError::InvalidPage
            } else {
                SandboxStoreError::Unavailable("usage changed while opening its snapshot".into())
            });
        }
    }
    let observed = session.snapshot_time().ok_or_else(|| {
        SandboxStoreError::Unavailable("usage read did not establish a snapshot".into())
    })?;
    Ok((
        rows,
        UsageSnapshot {
            time: observed.time,
            increment: observed.increment,
        },
    ))
}

fn project_page<T>(
    rows: Vec<Document>,
    limit: u16,
    read: impl Fn(&Document) -> Result<T, SandboxStoreError>,
    key: impl Fn(&T) -> String,
) -> Result<Page<T>, SandboxStoreError> {
    let mut items = rows.iter().map(read).collect::<Result<Vec<_>, _>>()?;
    let next = if items.len() > usize::from(limit) {
        items.pop();
        items.last().map(key)
    } else {
        None
    };
    Ok(Page { items, next })
}

fn validate_page(page: &PageRequest) -> Result<(), SandboxStoreError> {
    if page.limit == 0
        || page.limit > MAX_PAGE_SIZE
        || page
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.len() > 256)
    {
        Err(SandboxStoreError::InvalidPage)
    } else {
        Ok(())
    }
}
fn owner_filter(owner: Option<&SubjectId>) -> Document {
    owner.map_or_else(Document::new, |owner| doc! { "owner": owner.as_str() })
}
fn number(document: &Document, key: &str) -> Result<u64, SandboxStoreError> {
    let value = match document.get(key) {
        Some(Bson::Int32(value)) => i64::from(*value),
        Some(Bson::Int64(value)) => *value,
        _ => {
            return Err(SandboxStoreError::Corrupt(format!(
                "invalid usage quantity: {key}"
            )))
        }
    };
    u64::try_from(value)
        .map_err(|_| SandboxStoreError::Corrupt(format!("negative usage quantity: {key}")))
}
fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
