use super::*;
use std::{collections::BTreeMap, sync::Mutex};
#[derive(Default)]
struct Rows {
    latest: BTreeMap<String, EventWorkerProfile>,
    revisions: BTreeMap<(String, u32), EventWorkerProfile>,
}
#[derive(Default)]
pub struct InMemoryEventWorkerProfileStore {
    rows: Mutex<Rows>,
}
#[async_trait::async_trait]
impl EventWorkerProfileStore for InMemoryEventWorkerProfileStore {
    async fn get(
        &self,
        id: &str,
        revision: Option<u32>,
    ) -> Result<Option<EventWorkerProfile>, BotError> {
        let rows = self.rows.lock().map_err(|_| BotError::Unavailable)?;
        Ok(match revision {
            Some(n) => rows.revisions.get(&(id.into(), n)),
            None => rows.latest.get(id),
        }
        .cloned())
    }
    async fn list(
        &self,
        owner: &SubjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventWorkerProfile>, BotError> {
        let rows = self.rows.lock().map_err(|_| BotError::Unavailable)?;
        Ok(rows
            .latest
            .values()
            .filter(|v| {
                v.owner == *owner
                    && v.retired_at.is_none()
                    && after.is_none_or(|a| v.id.as_str() > a)
            })
            .take(limit)
            .cloned()
            .collect())
    }
    async fn save(
        &self,
        expected: Option<u32>,
        value: &EventWorkerProfile,
    ) -> Result<(), BotError> {
        value.validate()?;
        let mut rows = self.rows.lock().map_err(|_| BotError::Unavailable)?;
        if rows.latest.get(&value.id).map(|v| v.revision) != expected
            || expected.map_or(value.revision != 1, |n| {
                n.checked_add(1) != Some(value.revision)
            })
            || rows
                .latest
                .get(&value.id)
                .is_some_and(|v| v.owner != value.owner || v.retired_at.is_some())
        {
            return Err(BotError::Conflict);
        }
        if rows
            .revisions
            .contains_key(&(value.id.clone(), value.revision))
        {
            return Err(BotError::Conflict);
        }
        rows.revisions
            .insert((value.id.clone(), value.revision), value.clone());
        rows.latest.insert(value.id.clone(), value.clone());
        Ok(())
    }
}
