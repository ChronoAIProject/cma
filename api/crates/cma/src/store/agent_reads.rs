//! Mongo owner-agent filtering for the Layer 2 read model.

use layer2_core::ids::SubjectId;
use layer2_core::sandbox::store::{
    AgentReadQueryStore, Page, PageRequest, SandboxStoreError, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::{Agent, AgentListQuery};
use mongodb::bson::doc;

use super::agents::read_agent;
use super::sandbox_mongo::{record_bson, MongoSandboxStore};

#[async_trait::async_trait]
impl AgentReadQueryStore for MongoSandboxStore {
    async fn list_owned_agents(
        &self,
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

        let mut read = super::mongo_read_telemetry::Read::find(self.agents.name());
        let mut cursor = self
            .agents
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(document) = read
            .next(&mut cursor)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            items.push(read_agent(&document)?);
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
