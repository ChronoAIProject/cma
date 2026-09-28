//! Expiring opaque gateway state, isolated from durable channel definitions.
use std::sync::Arc;

use crate::{Caller, Capability, CoreError, GatewayRecord, StoreError, SubjectId};

pub const MAX_INTERACTION_LIFETIME_SECONDS: i64 = 7 * 24 * 60 * 60;

/// The gateway encrypts and interprets the payload. This port only enforces
/// retention and optimistic concurrency in a separate record namespace.
#[async_trait::async_trait]
pub trait GatewayInteractionStore: Send + Sync {
    async fn put(
        &self,
        record_id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected_version: Option<u64>,
        expires_at: i64,
    ) -> Result<GatewayRecord, StoreError>;
    async fn get(&self, record_id: &str) -> Result<Option<GatewayRecord>, StoreError>;
    async fn delete(&self, record_id: &str, expected_version: u64) -> Result<bool, StoreError>;
}

pub struct GatewayInteractionService {
    store: Arc<dyn GatewayInteractionStore>,
}

impl GatewayInteractionService {
    pub fn new(store: Arc<dyn GatewayInteractionStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }

    pub async fn put(
        &self,
        caller: &Caller,
        record_id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected_version: Option<u64>,
        expires_at: i64,
    ) -> Result<GatewayRecord, CoreError> {
        authorize(caller, record_id)?;
        crate::gateway_records::validate_blob(&blob)?;
        if owner.as_str().is_empty() || owner.as_str().len() > 512 {
            return Err(CoreError::InvalidRequest("invalid record owner".into()));
        }
        validate_expiry(expires_at, time::OffsetDateTime::now_utc().unix_timestamp())?;
        self.store
            .put(record_id, owner, blob, expected_version, expires_at)
            .await
            .map_err(crate::gateway_records::store_failed)
    }

    pub async fn get(
        &self,
        caller: &Caller,
        record_id: &str,
    ) -> Result<Option<GatewayRecord>, CoreError> {
        authorize(caller, record_id)?;
        self.store
            .get(record_id)
            .await
            .map_err(crate::gateway_records::store_failed)
    }

    pub async fn delete(
        &self,
        caller: &Caller,
        record_id: &str,
        expected_version: u64,
    ) -> Result<bool, CoreError> {
        authorize(caller, record_id)?;
        self.store
            .delete(record_id, expected_version)
            .await
            .map_err(crate::gateway_records::store_failed)
    }
}

fn authorize(caller: &Caller, record_id: &str) -> Result<(), CoreError> {
    caller.require(Capability::ManageGatewayRecords)?;
    crate::gateway_records::validate_record_id(record_id)
}

fn validate_expiry(expires_at: i64, now: i64) -> Result<(), CoreError> {
    if expires_at <= now || expires_at > now.saturating_add(MAX_INTERACTION_LIFETIME_SECONDS) {
        return Err(CoreError::InvalidRequest(
            "interaction expiry must be in the next seven days".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_retention_and_capability_are_required() {
        for expiry in [
            0,
            99,
            100,
            i64::MAX,
            100 + MAX_INTERACTION_LIFETIME_SECONDS + 1,
        ] {
            assert!(validate_expiry(expiry, 100).is_err());
        }
        assert!(validate_expiry(101, 100).is_ok());
        let id = uuid::Uuid::new_v4().to_string();
        let caller = Caller::new(SubjectId::new("actor"), crate::Authorization::none());
        assert_eq!(authorize(&caller, &id), Err(CoreError::Forbidden));
    }
}
