//! Opaque, capability-gated records used by the event gateway.

use std::sync::Arc;

use crate::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, Caller,
    Capability, CoreError, StoreError, SubjectId,
};

pub const MIN_GATEWAY_RECORD_BLOB_BYTES: usize = 1;
pub const MAX_GATEWAY_RECORD_BLOB_BYTES: usize = 65_536;
pub const MAX_GATEWAY_RECORD_PAGE_SIZE: usize = 200;

/// A CMA-owned opaque channel definition blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayRecord {
    pub record_id: String,
    pub owner: SubjectId,
    pub blob: Vec<u8>,
    pub version: u64,
    pub created_at: String,
    pub updated_at: String,
}

/// Metadata returned by owner-scoped record listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayRecordHead {
    pub record_id: String,
    pub owner: SubjectId,
    pub version: u64,
    pub updated_at: String,
}

/// Durable port for opaque event-gateway channel records.
#[async_trait::async_trait]
pub trait GatewayRecordStore: Send + Sync {
    async fn put(
        &self,
        record_id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected_version: Option<u64>,
    ) -> Result<GatewayRecord, StoreError>;

    async fn get(&self, record_id: &str) -> Result<Option<GatewayRecord>, StoreError>;

    async fn delete(
        &self,
        record_id: &str,
        expected_version: Option<u64>,
    ) -> Result<bool, StoreError>;

    async fn list(
        &self,
        owner: Option<&SubjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GatewayRecordHead>, StoreError>;
}

/// Authorization and activity boundary around the gateway record port.
pub struct GatewayRecordService {
    store: Arc<dyn GatewayRecordStore>,
    activity: Arc<ActivityService>,
}

impl GatewayRecordService {
    pub fn new(store: Arc<dyn GatewayRecordStore>, activity: Arc<ActivityService>) -> Arc<Self> {
        Arc::new(Self { store, activity })
    }

    pub async fn put(
        &self,
        caller: &Caller,
        record_id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected_version: Option<u64>,
    ) -> Result<GatewayRecord, CoreError> {
        authorize(caller)?;
        validate_record_id(record_id)?;
        validate_blob(&blob)?;
        let record = self
            .store
            .put(record_id, owner, blob, expected_version)
            .await
            .map_err(store_failed)?;
        self.audit(caller, ActivityAction::GatewayRecordWrite, record_id)
            .await?;
        Ok(record)
    }

    pub async fn get(
        &self,
        caller: &Caller,
        record_id: &str,
    ) -> Result<Option<GatewayRecord>, CoreError> {
        authorize(caller)?;
        validate_record_id(record_id)?;
        let record = self.store.get(record_id).await.map_err(store_failed)?;
        self.audit(caller, ActivityAction::GatewayRecordRead, record_id)
            .await?;
        Ok(record)
    }

    pub async fn delete(
        &self,
        caller: &Caller,
        record_id: &str,
        expected_version: Option<u64>,
    ) -> Result<bool, CoreError> {
        authorize(caller)?;
        validate_record_id(record_id)?;
        let deleted = self
            .store
            .delete(record_id, expected_version)
            .await
            .map_err(store_failed)?;
        self.audit(caller, ActivityAction::GatewayRecordDelete, record_id)
            .await?;
        Ok(deleted)
    }

    pub async fn list(
        &self,
        caller: &Caller,
        owner: Option<&SubjectId>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GatewayRecordHead>, CoreError> {
        authorize(caller)?;
        if !(1..=MAX_GATEWAY_RECORD_PAGE_SIZE).contains(&limit) {
            return Err(CoreError::InvalidRequest(format!(
                "gateway record limit must be between 1 and {MAX_GATEWAY_RECORD_PAGE_SIZE}"
            )));
        }
        if let Some(after) = after {
            validate_record_id(after)?;
        }
        let records = self
            .store
            .list(owner, after, limit)
            .await
            .map_err(store_failed)?;
        for record in &records {
            self.audit(caller, ActivityAction::GatewayRecordRead, &record.record_id)
                .await?;
        }
        Ok(records)
    }

    async fn audit(
        &self,
        caller: &Caller,
        action: ActivityAction,
        record_id: &str,
    ) -> Result<(), CoreError> {
        let record = ActivityRecord::new(action, ActivityOutcome::Succeeded)
            .by(caller.subject.clone())
            .on(ActivityTarget::Resource(record_id.to_owned()));
        self.activity
            .record(&record)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }
}

fn authorize(caller: &Caller) -> Result<(), CoreError> {
    caller.require(Capability::ManageGatewayRecords)
}

pub(super) fn validate_record_id(record_id: &str) -> Result<(), CoreError> {
    let uuid = uuid::Uuid::parse_str(record_id)
        .map_err(|_| CoreError::InvalidRequest("record_id must be a UUID v4".into()))?;
    if uuid.get_version_num() != 4 || uuid.hyphenated().to_string() != record_id {
        return Err(CoreError::InvalidRequest(
            "record_id must be a canonical UUID v4".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_blob(blob: &[u8]) -> Result<(), CoreError> {
    if !(MIN_GATEWAY_RECORD_BLOB_BYTES..=MAX_GATEWAY_RECORD_BLOB_BYTES).contains(&blob.len()) {
        return Err(CoreError::InvalidRequest(format!(
            "gateway record blob must be between {MIN_GATEWAY_RECORD_BLOB_BYTES} and {MAX_GATEWAY_RECORD_BLOB_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(super) fn store_failed(error: StoreError) -> CoreError {
    match error {
        StoreError::RevisionConflict { expected, actual } => CoreError::InvalidState(format!(
            "gateway record version conflict: expected {expected}, found {actual}"
        )),
        error => CoreError::RuntimeUnavailable(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Mutex};

    use super::*;

    struct MemoryStore(Mutex<BTreeMap<String, GatewayRecord>>);

    #[async_trait::async_trait]
    impl GatewayRecordStore for MemoryStore {
        async fn put(
            &self,
            record_id: &str,
            owner: &SubjectId,
            blob: Vec<u8>,
            expected_version: Option<u64>,
        ) -> Result<GatewayRecord, StoreError> {
            let mut records = self.0.lock().unwrap();
            let current = records.get(record_id).cloned();
            match (current, expected_version) {
                (None, None) => {
                    let record = GatewayRecord {
                        record_id: record_id.into(),
                        owner: owner.clone(),
                        blob,
                        version: 1,
                        created_at: "now".into(),
                        updated_at: "now".into(),
                    };
                    records.insert(record_id.into(), record.clone());
                    Ok(record)
                }
                (Some(mut record), Some(expected)) if record.version == expected => {
                    record.owner = owner.clone();
                    record.blob = blob;
                    record.version += 1;
                    records.insert(record_id.into(), record.clone());
                    Ok(record)
                }
                (Some(record), expected) => Err(StoreError::RevisionConflict {
                    expected: expected.unwrap_or(0),
                    actual: record.version,
                }),
                (None, Some(expected)) => Err(StoreError::RevisionConflict {
                    expected,
                    actual: 0,
                }),
            }
        }

        async fn get(&self, record_id: &str) -> Result<Option<GatewayRecord>, StoreError> {
            Ok(self.0.lock().unwrap().get(record_id).cloned())
        }

        async fn delete(
            &self,
            record_id: &str,
            expected_version: Option<u64>,
        ) -> Result<bool, StoreError> {
            let mut records = self.0.lock().unwrap();
            let Some(record) = records.get(record_id) else {
                return Ok(false);
            };
            if let Some(expected) = expected_version {
                if expected != record.version {
                    return Err(StoreError::RevisionConflict {
                        expected,
                        actual: record.version,
                    });
                }
            }
            records.remove(record_id);
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

    struct ActivityMemory;

    #[async_trait::async_trait]
    impl crate::ActivityStore for ActivityMemory {
        async fn append(&self, _: &ActivityRecord) -> Result<(), StoreError> {
            Ok(())
        }

        async fn list(
            &self,
            _: &crate::ActivityQuery,
        ) -> Result<crate::Page<ActivityRecord>, StoreError> {
            Ok(crate::Page::new(Vec::new(), None))
        }
    }

    fn caller() -> Caller {
        Caller::new(
            SubjectId::new("owner"),
            crate::Authorization::new([Capability::ManageGatewayRecords]),
        )
    }

    #[tokio::test]
    async fn service_enforces_cas_and_capability() {
        let service = GatewayRecordService::new(
            Arc::new(MemoryStore(Mutex::new(BTreeMap::new()))),
            ActivityService::new(Arc::new(ActivityMemory)),
        );
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SubjectId::new("owner");
        let stored = service
            .put(&caller(), &id, &owner, vec![1], None)
            .await
            .unwrap();
        assert_eq!(stored.version, 1);
        assert!(matches!(
            service.put(&caller(), &id, &owner, vec![2], Some(9)).await,
            Err(CoreError::InvalidState(_))
        ));
        assert!(service.get(&caller(), &id).await.unwrap().is_some());
        let denied = Caller::new(SubjectId::new("owner"), crate::Authorization::none());
        assert_eq!(service.get(&denied, &id).await, Err(CoreError::Forbidden));
    }
}
