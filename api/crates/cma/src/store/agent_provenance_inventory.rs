use crate::store::transaction_terminal::GuardedTerminal as _;
use std::time::Duration;

use layer2_core::resource_access::{ResourceAccessStoreError, ResourceOwner};
use layer2_core::sandbox::store::{
    AgentChannelUsageSnapshot, AgentProvenanceInventoryStore, PageRequest, SandboxStoreError,
};
use layer2_core::sandbox::AgentListQuery;
use mongodb::{bson::doc, ClientSession};

use super::{
    agents::read_agent,
    resource_access::{read_agent_inventory, read_shared_agent_inventory, ReadError},
    sandbox_mongo::{
        is_transient, record_bson, start_transaction, MongoSandboxStore, RESOURCE_POLICIES,
        TRANSACTION_ATTEMPTS,
    },
};

mod personal_batch;

#[async_trait::async_trait]
impl AgentProvenanceInventoryStore for MongoSandboxStore {
    async fn scan_agent_provenance_inventory(
        &self,
        scope: &ResourceOwner,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentChannelUsageSnapshot>, SandboxStoreError> {
        PageRequest::new(page.limit, page.cursor.clone())?;
        if self.resource_list_batch_reads {
            if let ResourceOwner::Person { subject } = scope {
                return personal_batch::read(self, subject, query, page).await;
            }
        }
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| unavailable())?;
            start_transaction(&mut session)
                .await
                .map_err(|_| unavailable())?;
            let result = scan(self, scope, query, page, &mut session).await;
            let _ = session.abort_guarded().await;
            match result {
                Ok(rows) => return Ok(rows),
                Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                Err(ReadError::Driver(_)) => return Err(unavailable()),
                Err(ReadError::Domain(error)) => {
                    return Err(SandboxStoreError::Corrupt(error.to_string()))
                }
            }
        }
        Err(unavailable())
    }
    async fn scan_shared_agent_provenance_inventory(
        &self,
        recipient: &layer2_core::SubjectId,
        organizations: &[layer2_core::SubjectId],
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Vec<AgentChannelUsageSnapshot>, SandboxStoreError> {
        PageRequest::new(page.limit, page.cursor.clone())?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| unavailable())?;
            start_transaction(&mut session)
                .await
                .map_err(|_| unavailable())?;
            let result = async {
                let candidates = read_shared_agent_inventory(
                    self,
                    recipient,
                    organizations,
                    query,
                    page,
                    &mut session,
                )
                .await?;
                let mut rows = Vec::with_capacity(candidates.len());
                for candidate in candidates {
                    let id = candidate
                        .lineage
                        .agent
                        .ok_or(ResourceAccessStoreError::Corrupt)?
                        .id;
                    rows.push(read_snapshot(self, &id, &mut session).await?);
                }
                read_statuses(self, &mut rows, &mut session).await?;
                Ok::<_, ReadError>(rows)
            }
            .await;
            let _ = session.abort_guarded().await;
            match result {
                Ok(rows) => return Ok(rows),
                Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                Err(ReadError::Driver(_)) => return Err(unavailable()),
                Err(ReadError::Domain(error)) => {
                    return Err(SandboxStoreError::Corrupt(error.to_string()))
                }
            }
        }
        Err(unavailable())
    }
}

async fn scan(
    store: &MongoSandboxStore,
    scope: &ResourceOwner,
    query: &AgentListQuery,
    page: &PageRequest,
    session: &mut ClientSession,
) -> Result<Vec<AgentChannelUsageSnapshot>, ReadError> {
    let mut ids = Vec::new();
    match scope {
        ResourceOwner::Organization { subject } => {
            for snapshot in read_agent_inventory(store, subject, query, page, session).await? {
                ids.push(
                    snapshot
                        .lineage
                        .agent
                        .ok_or(ResourceAccessStoreError::Corrupt)?
                        .id,
                );
            }
        }
        ResourceOwner::Person { subject } => {
            let pipeline = personal_selection(subject, query, page)?;
            let mut cursor = store
                .agents
                .aggregate(pipeline)
                .max_time(Duration::from_secs(10))
                .session(&mut *session)
                .await?;
            while cursor.advance(&mut *session).await? {
                let agent = read_agent(&cursor.deserialize_current()?)
                    .map_err(|_| ResourceAccessStoreError::Corrupt)?;
                ids.push(agent.id);
            }
        }
    }
    let mut rows = Vec::with_capacity(ids.len());
    for id in ids {
        rows.push(read_snapshot(store, &id, session).await?);
    }
    read_statuses(store, &mut rows, session).await?;
    Ok(rows)
}

fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("Agent provenance inventory is unavailable".into())
}

async fn read_snapshot(
    store: &MongoSandboxStore,
    id: &layer2_core::AgentId,
    session: &mut ClientSession,
) -> Result<AgentChannelUsageSnapshot, ReadError> {
    let snapshot = super::agent_channel_usage::read_inventory_snapshot(store, id, session)
        .await
        .map_err(|error| match error {
            super::agent_channel_usage::UsageError::Driver(error) => ReadError::Driver(error),
            super::agent_channel_usage::UsageError::Domain(error) => {
                ReadError::Domain(match error {
                    SandboxStoreError::NotFound => ResourceAccessStoreError::NotFound,
                    SandboxStoreError::Conflict => ResourceAccessStoreError::Conflict,
                    SandboxStoreError::Unavailable(_) => ResourceAccessStoreError::Unavailable,
                    _ => ResourceAccessStoreError::Corrupt,
                })
            }
        })?;
    Ok(snapshot)
}

async fn read_statuses(
    store: &MongoSandboxStore,
    rows: &mut [AgentChannelUsageSnapshot],
    session: &mut ClientSession,
) -> Result<(), ReadError> {
    let agents = rows
        .iter()
        .map(|row| {
            row.provenance
                .resource
                .lineage
                .agent
                .as_ref()
                .ok_or(ResourceAccessStoreError::Corrupt)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let statuses = super::agent_task_status::read_many(store, agents, session)
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)?;
    for row in rows {
        let agent = row
            .provenance
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        row.task_status = *statuses
            .get(&agent.id)
            .ok_or(ResourceAccessStoreError::Corrupt)?;
    }
    Ok(())
}

fn personal_selection(
    subject: &layer2_core::SubjectId,
    query: &AgentListQuery,
    page: &PageRequest,
) -> Result<Vec<mongodb::bson::Document>, ReadError> {
    let mut filter = doc! {"owner":subject.as_str()};
    if let Some(id) = &query.sandbox_id {
        filter.insert("sandbox_id", id.as_str());
    }
    if let Some(id) = &query.workspace_id {
        filter.insert("workspace_id", id.as_str());
    }
    if let Some(status) = query.status {
        filter.insert(
            "status",
            record_bson(&status).map_err(|_| ResourceAccessStoreError::Corrupt)?,
        );
    }
    if let Some(cursor) = &page.cursor {
        filter.insert("_id", doc! {"$gt":cursor});
    }
    Ok(vec![
        doc! {"$match":filter},
        doc! {"$lookup":{"from":RESOURCE_POLICIES,"localField":"sandbox_id","foreignField":"_id","as":"inventory_policy"}},
        doc! {"$match":{"$or":[{"inventory_policy":{"$size":0}}, {"inventory_policy":{"$elemMatch":{"owner_kind":"person","owner_subject":subject.as_str()}}}]}},
        doc! {"$sort":{"_id":1}},
        doc! {"$limit":i64::from(page.limit)+1},
        doc! {"$unset":"inventory_policy"},
    ])
}
