use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::{Page, PageRequest, MAX_PAGE_SIZE};
use layer2_core::sandbox_usage::UsageTotals;
use layer2_core::sandbox_usage_service::{SandboxUsageStore, UsageSummary};
use layer2_core::{SandboxId, SubjectId};
use mongodb::bson::Bson;

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
        validate_page(page)?;
        let mut filter = owner_filter(owner);
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut cursor = self
            .sandbox_usage
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(unavailable)?;
        let mut items = Vec::new();
        while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
            items.push(read_state(&document)?);
        }
        let next = if items.len() > usize::from(page.limit) {
            items.pop();
            items.last().map(|state| state.sandbox_id.to_string())
        } else {
            None
        };
        Ok(Page { items, next })
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
        let mut cursor = self
            .sandbox_usage_events
            .find(filter)
            .sort(doc! { "revision": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(unavailable)?;
        let mut items = Vec::new();
        while let Some(document) = cursor.try_next().await.map_err(unavailable)? {
            items.push(read_event(&document)?);
        }
        let next = if items.len() > usize::from(page.limit) {
            items.pop();
            items.last().map(|event| event.revision.to_string())
        } else {
            None
        };
        Ok(Page { items, next })
    }
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
