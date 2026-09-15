use std::collections::BTreeMap;

use crate::resource_access::ResourceOwner;

use super::{
    match_bridge_replay, BridgeError, BridgeId, BridgeRecord, BridgeStore, BridgeWrite,
    StoredBridgeReceipt,
};

#[derive(Default)]
struct State {
    records: BTreeMap<BridgeId, BridgeRecord>,
    receipts: BTreeMap<String, StoredBridgeReceipt>,
}

#[derive(Default)]
pub struct MemoryBridgeStore {
    state: tokio::sync::Mutex<State>,
}

#[async_trait::async_trait]
impl BridgeStore for MemoryBridgeStore {
    async fn bridge(&self, id: &BridgeId) -> Result<Option<BridgeRecord>, BridgeError> {
        let result = self.state.lock().await.records.get(id).cloned();
        if let Some(record) = &result {
            record.validate()?;
        }
        Ok(result)
    }

    async fn receipt(&self, id: &str) -> Result<Option<StoredBridgeReceipt>, BridgeError> {
        Ok(self.state.lock().await.receipts.get(id).cloned())
    }

    async fn list(
        &self,
        owner: &ResourceOwner,
        issuer: &str,
        after: Option<&BridgeId>,
        limit: u32,
    ) -> Result<Vec<BridgeRecord>, BridgeError> {
        if !(1..=100).contains(&limit) {
            return Err(BridgeError::InvalidConfiguration("limit"));
        }
        let state = self.state.lock().await;
        let records = state
            .records
            .values()
            .filter(|record| {
                &record.owner == owner
                    && record
                        .current()
                        .is_ok_and(|g| g.config.connector.issuer == issuer)
                    && after.is_none_or(|id| &record.id > id)
            })
            .take(limit as usize + 1)
            .cloned()
            .collect::<Vec<_>>();
        for record in &records {
            record.validate()?;
        }
        Ok(records)
    }

    async fn apply(&self, write: BridgeWrite) -> Result<StoredBridgeReceipt, BridgeError> {
        let mut state = self.state.lock().await;
        write.require_current()?;
        if let Some(receipt) = state.receipts.get(&write.receipt().id) {
            return match_bridge_replay(receipt, write.receipt());
        }
        // This standalone test adapter has no common grant/resource transaction.
        // It cannot promise an atomic fixed-target enable fence.
        if write.enable_grant().is_some() {
            return Err(BridgeError::Unavailable);
        }
        let current = state.records.get(&write.record().id);
        if current.map_or(0, |r| r.revision) != write.expected_revision()
            || current.is_some_and(|r| r.owner != write.record().owner)
        {
            return Err(BridgeError::Conflict);
        }
        state
            .records
            .insert(write.record().id.clone(), write.record().clone());
        state
            .receipts
            .insert(write.receipt().id.clone(), write.receipt().clone());
        Ok(write.receipt().clone())
    }
}
