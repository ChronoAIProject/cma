//! Child enumeration in the same exclusive snapshot as captured log evidence.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, Workspace};
use layer2_core::SandboxId;
use mongodb::{
    bson::{doc, Document},
    Collection,
};

use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::transaction_scope::TransactionScope, agents::read_agent,
    sandbox_mongo::MongoSandboxStore, workspaces::read_workspace,
};

pub(in crate::store) async fn agents(
    store: &MongoSandboxStore,
    id: &SandboxId,
    scope: &mut TransactionScope<'_>,
) -> Result<Vec<Agent>, TransactionLogError> {
    rows(&store.agents, id, scope, read_agent).await
}

pub(in crate::store) async fn workspaces(
    store: &MongoSandboxStore,
    id: &SandboxId,
    scope: &mut TransactionScope<'_>,
) -> Result<Vec<Workspace>, TransactionLogError> {
    rows(&store.workspaces, id, scope, read_workspace).await
}

async fn rows<T>(
    collection: &Collection<Document>,
    id: &SandboxId,
    scope: &mut TransactionScope<'_>,
    decode: impl Fn(&Document) -> Result<T, SandboxStoreError>,
) -> Result<Vec<T>, TransactionLogError> {
    const PAGE: u32 = 128;
    let mut rows = Vec::new();
    let mut budget = Budget::default();
    let mut after: Option<String> = None;
    loop {
        let mut filter = doc! {"sandbox_id": id.as_str()};
        if let Some(after) = &after {
            filter.insert("_id", doc! {"$gt": after});
        }
        let page = scope
            .find_page(collection, filter, doc! {"_id": 1}, PAGE)
            .await
            .map_err(TransactionLogError::Driver)?;
        if page.len() > PAGE as usize {
            return Err(SandboxStoreError::Conflict.into());
        }
        let complete = page.len() < PAGE as usize;
        for row in page {
            budget.record(&row)?;
            let key = row
                .get_str("_id")
                .map_err(|_| SandboxStoreError::Conflict)?;
            if after
                .as_ref()
                .is_some_and(|previous| key <= previous.as_str())
            {
                return Err(SandboxStoreError::Conflict.into());
            }
            after = Some(key.to_owned());
            rows.push(decode(&row)?);
        }
        if complete {
            return Ok(rows);
        }
    }
}

// Bounds apply to the entire child set, across every query page. Callers get
// either the complete set or an error before they can use it for any mutation.
const MAX_CHILDREN: usize = 512;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct Budget {
    count: usize,
    bytes: usize,
}
impl Budget {
    fn record(&mut self, row: &Document) -> Result<(), SandboxStoreError> {
        let bytes = mongodb::bson::serialize_to_vec(row)
            .map_err(|_| SandboxStoreError::Conflict)?
            .len();
        let total = self
            .bytes
            .checked_add(bytes)
            .filter(|total| *total <= MAX_BYTES);
        if self.count >= MAX_CHILDREN || total.is_none() {
            return Err(SandboxStoreError::Unavailable(
                "child enumeration exceeds its total count or BSON byte budget".into(),
            ));
        }
        self.count += 1;
        self.bytes = total.expect("budget checked");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_budget_spans_pages_and_refuses_before_accumulation() {
        let mut budget = Budget::default();
        for index in 0..MAX_CHILDREN {
            budget.record(&doc! {"_id": index as i64}).unwrap();
        }
        let accepted_bytes = budget.bytes;
        assert!(budget.record(&doc! {"_id": "overflow"}).is_err());
        assert_eq!(budget.count, MAX_CHILDREN);
        assert_eq!(budget.bytes, accepted_bytes);
        let row = doc! {"payload": "x".repeat(32 * 1024)};
        let mut budget = Budget::default();
        let mut accepted = 0;
        while budget.record(&row).is_ok() {
            accepted += 1;
        }
        assert!(accepted > 128 && accepted < MAX_CHILDREN);
        assert!(budget.bytes <= MAX_BYTES);
    }
}
