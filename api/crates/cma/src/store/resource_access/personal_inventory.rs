//! One fresh snapshot fences every selected personal Agent before release.
use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, MAX_ACTIVE_RESOURCE_SHARES,
};
use layer2_core::sandbox::store::MAX_PAGE_SIZE;
use layer2_core::sandbox::Agent;
use mongodb::bson::{doc, Document};
use std::time::Duration;

use super::ReadError;
use crate::store::sandbox_mongo::MongoSandboxStore;

mod decode;
pub(super) mod parents;
pub(in crate::store) mod query;

impl MongoSandboxStore {
    pub(crate) fn with_resource_list_batch_reads(mut self, enabled: bool) -> Self {
        self.resource_list_batch_reads = enabled;
        self
    }
}

pub(super) async fn fence(
    store: &MongoSandboxStore,
    agents: &[Agent],
) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
    if agents.is_empty() {
        return Ok(Vec::new());
    }
    if agents.len() > usize::from(MAX_PAGE_SIZE) + 1
        || agents
            .windows(2)
            .any(|pair| pair[0].id.as_str() >= pair[1].id.as_str())
    {
        return Err(ResourceAccessStoreError::Invalid);
    }
    let rows = read(store, agents).await.map_err(error)?;
    if rows.len() != agents.len()
        || rows.iter().zip(agents).any(|(row, selected)| {
            row.lineage
                .agent
                .as_ref()
                .is_none_or(|agent| agent != selected)
        })
    {
        return Err(ResourceAccessStoreError::Conflict);
    }
    Ok(rows)
}

async fn read(
    store: &MongoSandboxStore,
    agents: &[Agent],
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    let mut session = store.client.start_session().snapshot(true).await?;
    read_with_sources(store, agents, &mut session, Vec::new(), |_, _| {
        Err(ResourceAccessStoreError::Corrupt)
    })
    .await
}

pub(in crate::store) async fn read_with_sources(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut mongodb::ClientSession,
    extra: Vec<Document>,
    consume: impl FnMut(&str, Document) -> Result<(), ResourceAccessStoreError>,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    read_sources(store, agents, session, false, extra, consume).await
}

/// Reuse only Agents decoded from a prior query in the supplied snapshot.
/// This is source reuse within one capture, never a fresh authorization fence.
pub(in crate::store) async fn read_captured_sources(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut mongodb::ClientSession,
    extra: Vec<Document>,
    consume: impl FnMut(&str, Document) -> Result<(), ResourceAccessStoreError>,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    read_sources(store, agents, session, true, extra, consume).await
}

#[tracing::instrument(
    level = "debug",
    target = "cma.agent_inventory",
    name = "sources",
    skip_all
)]
async fn read_sources(
    store: &MongoSandboxStore,
    agents: &[Agent],
    session: &mut mongodb::ClientSession,
    captured: bool,
    extra: Vec<Document>,
    mut consume: impl FnMut(&str, Document) -> Result<(), ResourceAccessStoreError>,
) -> Result<Vec<ResourceAccessSnapshot>, ReadError> {
    let max_rows = agents.len();
    let collection = crate::store::snapshot_collection::collection(&store.agents, true);
    let mut pipeline = query::plan(store, agents, captured);
    pipeline.extend(extra);
    let mut cursor = collection
        .aggregate(pipeline)
        // A full small-record page fits in the initial reply. Mongo still
        // enforces its byte ceiling; getMore is counted by the route tests.
        .batch_size((max_rows * (10 + 4 * (MAX_ACTIVE_RESOURCE_SHARES + 1)) + 1024) as u32)
        .max_time(Duration::from_secs(3))
        .comment(if captured {
            "cma.personal_agent_evidence"
        } else {
            "cma.personal_agent_inventory"
        })
        .session(&mut *session)
        .await?;
    let mut sources = if captured {
        decode::Sources::captured(agents)
    } else {
        decode::Sources::default()
    };
    let mut kind = None;
    let mut telemetry = crate::store::mongo_read_telemetry::Read::find(collection.name());
    let mut insert = |kind: String, document| match decode::Kind::parse(&kind) {
        Ok(kind) => sources.insert(kind, document, max_rows),
        Err(_) => consume(&kind, document),
    };
    while let Some(mut document) = telemetry.next_in_session(&mut cursor, session).await? {
        if let Some(source_kind) = kind.take() {
            insert(source_kind, document)?;
        } else {
            let value = document.remove("value");
            if document.len() != 1 {
                return Err(ResourceAccessStoreError::Corrupt.into());
            }
            let source_kind = document
                .get_str("personal_inventory_source")
                .map_err(|_| ResourceAccessStoreError::Corrupt)?
                .to_owned();
            match value {
                Some(mongodb::bson::Bson::Document(source)) => insert(source_kind, source)?,
                None => kind = Some(source_kind),
                _ => return Err(ResourceAccessStoreError::Corrupt.into()),
            }
        }
    }
    if kind.is_some() {
        return Err(ResourceAccessStoreError::Corrupt.into());
    }
    let result = sources.page()?;
    telemetry.complete();
    Ok(result)
}

fn error(error: ReadError) -> ResourceAccessStoreError {
    match error {
        ReadError::Domain(error) => error,
        ReadError::Driver(_) => ResourceAccessStoreError::Unavailable,
    }
}
