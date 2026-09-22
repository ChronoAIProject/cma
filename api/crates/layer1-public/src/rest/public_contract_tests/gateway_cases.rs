use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use layer2_core::{
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, Capability,
    GatewayRecord, GatewayRecordHead, GatewayRecordService, GatewayRecordStore, Page, StoreError,
    SubjectId,
};

use super::Scenario;

#[derive(Default)]
struct Records(Mutex<BTreeMap<String, GatewayRecord>>);

#[async_trait]
impl GatewayRecordStore for Records {
    async fn put(
        &self,
        id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected: Option<u64>,
    ) -> Result<GatewayRecord, StoreError> {
        let mut records = self.0.lock().unwrap();
        if let Some(expected) = expected {
            let record = records.get_mut(id).ok_or(StoreError::RevisionConflict {
                expected,
                actual: 0,
            })?;
            if record.version != expected {
                return Err(StoreError::RevisionConflict {
                    expected,
                    actual: record.version,
                });
            }
            record.owner = owner.clone();
            record.blob = blob;
            record.version += 1;
            return Ok(record.clone());
        }
        if records.contains_key(id) {
            return Err(StoreError::RevisionConflict {
                expected: 0,
                actual: 1,
            });
        }
        let record = GatewayRecord {
            record_id: id.into(),
            owner: owner.clone(),
            blob,
            version: 1,
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        records.insert(id.into(), record.clone());
        Ok(record)
    }

    async fn get(&self, id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }

    async fn delete(&self, id: &str, expected: Option<u64>) -> Result<bool, StoreError> {
        let mut records = self.0.lock().unwrap();
        let Some(record) = records.get(id) else {
            return Ok(false);
        };
        if let Some(expected) = expected.filter(|version| *version != record.version) {
            return Err(StoreError::RevisionConflict {
                expected,
                actual: record.version,
            });
        }
        records.remove(id);
        Ok(true)
    }

    async fn list(
        &self,
        owner: Option<&SubjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GatewayRecordHead>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .values()
            .filter(|record| owner.is_none_or(|owner| owner == &record.owner))
            .filter(|record| after.is_none_or(|after| record.record_id.as_str() > after))
            .take(limit)
            .map(|record| GatewayRecordHead {
                record_id: record.record_id.clone(),
                owner: record.owner.clone(),
                version: record.version,
                updated_at: record.updated_at.clone(),
            })
            .collect())
    }
}

struct Activity;

#[async_trait]
impl ActivityStore for Activity {
    async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }
    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    (name == "gateway_records").then(|| {
        let service = GatewayRecordService::new(
            Arc::new(Records::default()),
            ActivityService::new(Arc::new(Activity)),
        );
        Scenario {
            router: crate::rest::gateway_record_router(service),
            subject: Some("gateway-owner"),
            authorization: Authorization::new([Capability::ManageGatewayRecords]),
            symbols: BTreeMap::new(),
        }
    })
}
