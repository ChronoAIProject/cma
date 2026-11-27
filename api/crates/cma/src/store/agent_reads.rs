//! Mongo owner-agent filtering for the Layer 2 read model.

use layer2_core::ids::SubjectId;
use layer2_core::sandbox::store::{
    AgentReadQueryStore, Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::{Agent, AgentListQuery};
use mongodb::{
    bson::{doc, Document},
    Collection,
};

use super::agents::read_agent_owned;
use super::sandbox_mongo::{record_bson, MongoSandboxStore};

#[async_trait::async_trait]
impl AgentReadQueryStore for MongoSandboxStore {
    async fn retained_agent_output(
        &self,
        agent: &Agent,
    ) -> Result<Option<layer2_core::sandbox::store::AgentMessageCheckpoint>, SandboxStoreError>
    {
        // The existing sidecar is at most 8 MiB (1 MiB of text). Reading it
        // whole preserves checkpoint validation without a new persisted format.
        let row = self.agents.find_one(doc! {
            "_id": agent.id.as_str(), "record.created_at": &agent.created_at,
            "owner": agent.owner.as_str(), "worktree_id": agent.worktree_id.as_str(),
            "workspace_id": agent.workspace_id.as_str(), "sandbox_id": agent.sandbox_id.as_str(),
        }).projection(doc! {"agent_capacity_prefix_v1": 1}).await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
            .ok_or(SandboxStoreError::NotFound)?;
        row.get("agent_capacity_prefix_v1")
            .map(|value| {
                let prefix: layer2_core::sandbox::store::AgentMessageCheckpoint =
                    mongodb::bson::deserialize_from_bson(value.clone())
                        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
                prefix.validate()?;
                Ok(prefix)
            })
            .transpose()
    }

    async fn list_owned_agents(
        &self,
        owner: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        query_owned_agents(
            &self.agents,
            self.resource_list_batch_reads,
            owner,
            query,
            page,
        )
        .await
    }
}

#[tracing::instrument(
    level = "debug",
    target = "cma.agent_inventory",
    name = "selection",
    skip_all
)]
pub(super) async fn query_owned_agents(
    agents: &Collection<Document>,
    batch_reads: bool,
    owner: &SubjectId,
    query: &AgentListQuery,
    page: &PageRequest,
) -> Result<Page<Agent>, SandboxStoreError> {
    if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
        return Err(SandboxStoreError::InvalidPage);
    }
    let mut filter = doc! { "owner": owner.as_str() };
    if let Some(id) = &query.sandbox_id {
        filter.insert("sandbox_id", id.as_str());
    }
    if let Some(id) = &query.workspace_id {
        filter.insert("workspace_id", id.as_str());
    }
    if let Some(status) = query.status {
        let status = record_bson(&status).map_err(SandboxStoreError::Corrupt)?;
        filter.insert("status", status);
    }
    if let Some(cursor) = &page.cursor {
        filter.insert("_id", doc! { "$gt": cursor });
    }

    let mut read = super::mongo_read_telemetry::Read::find(agents.name());
    let mut find = agents
        .find(filter)
        .sort(doc! { "_id": 1 })
        .limit(i64::from(page.limit) + 1);
    if batch_reads {
        find = find.batch_size(u32::from(page.limit) + 2);
    }
    let mut cursor = find
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let mut items = Vec::new();
    while let Some(document) = read
        .next(&mut cursor)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
    {
        items.push(
            tracing::debug_span!(target: "cma.agent_inventory", "record_decode")
                .in_scope(|| read_agent_owned(document))?,
        );
    }
    let has_more = items.len() > usize::from(page.limit);
    if has_more {
        items.pop();
    }
    let next = has_more.then(|| {
        items
            .last()
            .expect("a bounded page has a last item")
            .id
            .to_string()
    });
    read.complete();
    Ok(Page { items, next })
}

#[cfg(test)]
mod tests {
    use layer2_core::sandbox::AgentStatus;
    use mongodb::bson::Bson;

    use super::*;

    #[test]
    fn status_filter_uses_the_stored_wire_value() {
        assert_eq!(
            record_bson(&AgentStatus::WaitingApproval).unwrap(),
            Bson::String("waiting_approval".into())
        );
    }
}
