//! Deterministic HTTP fixture storage. Real cross-collection fencing is tested
//! by the disposable Mongo source-control suite, not this transport adapter.
use layer2_core::{bridge::*, resource_access::ResourceOwner};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Default)]
pub(super) struct Store(
    Mutex<(
        BTreeMap<BridgeId, BridgeRecord>,
        BTreeMap<String, StoredBridgeReceipt>,
    )>,
);
#[async_trait::async_trait]
impl BridgeStore for Store {
    async fn bridge(&self, id: &BridgeId) -> Result<Option<BridgeRecord>, BridgeError> {
        let record = self.0.lock().unwrap().0.get(id).cloned();
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }
    async fn receipt(&self, id: &str) -> Result<Option<StoredBridgeReceipt>, BridgeError> {
        Ok(self.0.lock().unwrap().1.get(id).cloned())
    }
    async fn list(
        &self,
        owner: &ResourceOwner,
        issuer: &str,
        after: Option<&BridgeId>,
        limit: u32,
    ) -> Result<Vec<BridgeRecord>, BridgeError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .0
            .values()
            .filter(|record| {
                &record.owner == owner
                    && record
                        .current()
                        .is_ok_and(|generation| generation.config.connector.issuer == issuer)
                    && after.is_none_or(|after| &record.id > after)
            })
            .take(limit as usize + 1)
            .cloned()
            .collect())
    }
    async fn apply(&self, write: BridgeWrite) -> Result<StoredBridgeReceipt, BridgeError> {
        write.require_current()?;
        let mut state = self.0.lock().unwrap();
        if let Some(prior) = state.1.get(&write.receipt().id) {
            return match_bridge_replay(prior, write.receipt());
        }
        if state
            .0
            .get(&write.record().id)
            .map_or(0, |record| record.revision)
            != write.expected_revision()
        {
            return Err(BridgeError::Conflict);
        }
        state
            .0
            .insert(write.record().id.clone(), write.record().clone());
        state
            .1
            .insert(write.receipt().id.clone(), write.receipt().clone());
        Ok(write.receipt().clone())
    }
}
