//! Mongo operator inventory for sandbox aggregates.

use futures_util::TryStreamExt;
use layer2_core::ids::SandboxId;
use layer2_core::sandbox::store::{Page, SandboxEventStore, SandboxStoreError, MAX_PAGE_SIZE};
use layer2_core::sandbox::{
    AgentAdminSummary, SandboxAdminQuery, SandboxAdminStore, SandboxAdminSummary, SandboxAdminView,
    WorkspaceAdminSummary,
};
use mongodb::bson::{doc, Bson, Document};

use super::agents::read_agent;
use super::sandbox_mongo::{unsigned, MongoSandboxStore, AGENTS};
use super::sandboxes::read_sandbox;
use super::workspaces::read_workspace;

const AGENT_COUNTS: &str = "operator_agent_counts";

fn admin_filter(query: &SandboxAdminQuery) -> Document {
    let mut filter = Document::new();
    if let Some(owner) = &query.owner {
        filter.insert("owner", owner.as_str());
    }
    if let Some(status) = query.status {
        filter.insert("status", status.name());
    }
    if let Some(runtime_version) = &query.runtime_version {
        filter.insert("runtime_version", runtime_version);
    }
    if let Some(always_on) = query.always_on {
        filter.insert(
            "record.profile.always_on",
            if always_on {
                Bson::Boolean(true)
            } else {
                Bson::Document(doc! { "$ne": true })
            },
        );
    }
    if let Some(cursor) = &query.page.cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }
    filter
}

fn admin_pipeline(query: &SandboxAdminQuery) -> Vec<Document> {
    vec![
        doc! { "$match": super::sandbox_quarantine::exclude(admin_filter(query)) },
        doc! { "$sort": { "_id": 1 } },
        doc! { "$limit": i64::from(query.page.limit) + 1 },
        doc! {
            "$lookup": {
                "from": AGENTS,
                "let": { "sandbox_id": "$_id" },
                "pipeline": [
                    { "$match": { "$expr": { "$and": [
                        { "$eq": ["$sandbox_id", "$$sandbox_id"] },
                        { "$ne": ["$status", "deleted"] },
                    ] } } },
                    { "$count": "value" },
                ],
                "as": AGENT_COUNTS,
            }
        },
    ]
}

fn live_agent_count(document: &Document) -> Result<u64, SandboxStoreError> {
    let counts = document
        .get_array(AGENT_COUNTS)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let value = match counts.as_slice() {
        [] => return Ok(0),
        [Bson::Document(count)] => count
            .get("value")
            .ok_or_else(|| SandboxStoreError::Corrupt("agent count has no value".into()))?,
        _ => {
            return Err(SandboxStoreError::Corrupt(
                "agent count aggregation returned an invalid result".into(),
            ));
        }
    };
    match value {
        Bson::Int32(value) => unsigned(i64::from(*value), "live agent count"),
        Bson::Int64(value) => unsigned(*value, "live agent count"),
        _ => Err("live agent count is not an integer".into()),
    }
    .map_err(SandboxStoreError::Corrupt)
}

async fn read_workspaces(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<Vec<WorkspaceAdminSummary>, SandboxStoreError> {
    let mut cursor = store
        .workspaces
        .find(doc! { "sandbox_id": id.as_str() })
        .sort(doc! { "_id": 1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let mut rows = Vec::new();
    while let Some(document) = cursor
        .try_next()
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        rows.push(WorkspaceAdminSummary::from(&read_workspace(&document)?));
    }
    Ok(rows)
}

async fn read_agents(
    store: &MongoSandboxStore,
    id: &SandboxId,
) -> Result<Vec<AgentAdminSummary>, SandboxStoreError> {
    let mut read = super::mongo_read_telemetry::Read::find(store.agents.name());
    let mut cursor = store
        .agents
        .find(doc! { "sandbox_id": id.as_str() })
        .sort(doc! { "_id": 1 })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let mut rows = Vec::new();
    while let Some(document) = read
        .next(&mut cursor)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        rows.push(AgentAdminSummary::from(&read_agent(&document)?));
    }
    read.complete();
    Ok(rows)
}

#[async_trait::async_trait]
impl SandboxAdminStore for MongoSandboxStore {
    async fn list_admin_sandboxes(
        &self,
        query: &SandboxAdminQuery,
    ) -> Result<Page<SandboxAdminSummary>, SandboxStoreError> {
        if query.page.limit == 0 || query.page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut cursor = self
            .sandboxes
            .aggregate(admin_pipeline(query))
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            let count = live_agent_count(&document)?;
            items.push(SandboxAdminSummary::from_sandbox(
                &read_sandbox(&document)?,
                count,
            ));
        }
        let has_more = items.len() > usize::from(query.page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("a bounded page has a last sandbox")
                .id
                .to_string()
        });
        Ok(Page { items, next })
    }

    async fn read_admin_sandbox(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxAdminView>, SandboxStoreError> {
        let Some(document) = self
            .sandboxes
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        else {
            return Ok(None);
        };
        let sandbox = read_sandbox(&document)?;
        let (workspaces, agents) =
            tokio::try_join!(read_workspaces(self, id), read_agents(self, id))?;
        Ok(Some(SandboxAdminView {
            sandbox,
            workspaces,
            agents,
        }))
    }

    async fn read_admin_sandbox_events(
        &self,
        id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<layer2_core::sandbox::SandboxEvent>, SandboxStoreError> {
        let Some(document) = self
            .sandboxes
            .find_one(doc! { "_id": id.as_str() })
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        else {
            return Err(SandboxStoreError::NotFound);
        };
        read_sandbox(&document)?;
        <Self as SandboxEventStore>::sandbox_events(self, id, after, limit).await
    }
}

#[cfg(test)]
mod tests {
    use layer2_core::ids::SubjectId;
    use layer2_core::sandbox::store::PageRequest;
    use layer2_core::sandbox::SandboxStatus;

    use super::*;

    #[test]
    fn filters_and_agent_count_pipeline_are_exact() {
        let query = SandboxAdminQuery {
            owner: Some(SubjectId("operator-target".into())),
            status: Some(SandboxStatus::Paused),
            runtime_version: Some("runtime-7".into()),
            always_on: Some(true),
            page: PageRequest {
                limit: 25,
                cursor: Some("sbx_after".into()),
            },
        };
        let pipeline = admin_pipeline(&query);
        assert_eq!(
            pipeline[0],
            doc! { "$match": {
                "owner": "operator-target",
                "status": "paused",
                "runtime_version": "runtime-7",
                "record.profile.always_on": true,
                "_id": { "$gt": "sbx_after" },
                (super::super::sandbox_quarantine::FIELD): { "$exists": false },
            } }
        );
        assert_eq!(pipeline[1], doc! { "$sort": { "_id": 1 } });
        assert_eq!(pipeline[2], doc! { "$limit": 26_i64 });
        assert_eq!(
            pipeline[3],
            doc! { "$lookup": {
                "from": AGENTS,
                "let": { "sandbox_id": "$_id" },
                "pipeline": [
                    { "$match": { "$expr": { "$and": [
                        { "$eq": ["$sandbox_id", "$$sandbox_id"] },
                        { "$ne": ["$status", "deleted"] },
                    ] } } },
                    { "$count": "value" },
                ],
                "as": AGENT_COUNTS,
            } }
        );
    }

    #[test]
    fn computed_agent_count_accepts_empty_and_integer_results() {
        assert_eq!(live_agent_count(&doc! { AGENT_COUNTS: [] }).unwrap(), 0);
        assert_eq!(
            live_agent_count(&doc! { AGENT_COUNTS: [{ "value": 3_i64 }] }).unwrap(),
            3
        );
        assert!(live_agent_count(&doc! { AGENT_COUNTS: [{ "value": "three" }] }).is_err());
    }
}
