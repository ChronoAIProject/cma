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
    rows(&store.agents, id, scope)
        .await?
        .iter()
        .map(read_agent)
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

pub(in crate::store) async fn workspaces(
    store: &MongoSandboxStore,
    id: &SandboxId,
    scope: &mut TransactionScope<'_>,
) -> Result<Vec<Workspace>, TransactionLogError> {
    rows(&store.workspaces, id, scope)
        .await?
        .iter()
        .map(read_workspace)
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

async fn rows(
    collection: &Collection<Document>,
    id: &SandboxId,
    scope: &mut TransactionScope<'_>,
) -> Result<Vec<Document>, TransactionLogError> {
    const PAGE: u32 = 128;
    let mut rows = Vec::new();
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
        for row in &page {
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
        }
        let complete = page.len() < PAGE as usize;
        rows.extend(page);
        if complete {
            return Ok(rows);
        }
    }
}
