use layer1_public::rest::PersonalAgentInventory;
use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceOwner, ResourceTarget,
};
use layer2_core::sandbox::store::{Page, PageRequest, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentListQuery, AgentReadError};
use layer2_core::{Caller, Capability, CoreError, LaunchId, SubjectId};
use mongodb::bson::{doc, Document};

use super::MongoAccountStores;
use crate::store::{
    agent_reads::query_owned_agents,
    durability::majority_collection,
    resource_access::{
        snapshot_lineage::{read_from, LineageCollections},
        ReadError,
    },
    sandbox_mongo::{AGENTS, RESOURCE_POLICIES, SANDBOXES, WORKSPACES, WORKTREES},
    trigger_definitions::{decode_launch_aggregate, LAUNCHES},
};

impl MongoAccountStores {
    pub(crate) async fn owns_launch(
        &self,
        owner: &SubjectId,
        id: &LaunchId,
    ) -> Result<bool, CoreError> {
        let store_error =
            || CoreError::RuntimeUnavailable("Launch authorization is unavailable".into());
        let row = majority_collection::<Document>(&self.database, LAUNCHES)
            .find_one(doc! {"_id": id.as_str(), "owner": owner.as_str()})
            .await
            .map_err(|_| store_error())?;
        let Some(row) = row else {
            return Ok(false);
        };
        let launch = decode_launch_aggregate(&row).map_err(|_| store_error())?;
        Ok(launch.owner == *owner && launch.id == *id)
    }

    pub(crate) async fn personal_resource(
        &self,
        caller: &Caller,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        let agents = majority_collection(&self.database, AGENTS);
        let mut session = agents
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?;
        let (lineage, policy) = read_from(
            LineageCollections {
                agents: &agents,
                worktrees: &majority_collection(&self.database, WORKTREES),
                workspaces: &majority_collection(&self.database, WORKSPACES),
                sandboxes: &majority_collection(&self.database, SANDBOXES),
                resource_policies: &majority_collection(&self.database, RESOURCE_POLICIES),
            },
            target,
            &mut session,
            true,
        )
        .await
        .map_err(|error| match error {
            ReadError::Domain(error) => error,
            ReadError::Driver(_) => ResourceAccessStoreError::Unavailable,
        })?;
        let snapshot = ResourceAccessSnapshot {
            lineage,
            policy,
            shares: Vec::new(),
        };
        let scope = snapshot.scope(target)?;
        if !matches!(&scope.owner, ResourceOwner::Person { subject } if subject == &caller.subject)
        {
            return Err(ResourceAccessStoreError::NotFound);
        }
        Ok(snapshot)
    }
}

#[async_trait::async_trait]
impl PersonalAgentInventory for MongoAccountStores {
    async fn list(
        &self,
        caller: &Caller,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<Agent>, AgentReadError> {
        caller
            .require(Capability::UseOwnSandboxes)
            .map_err(|_| AgentReadError::Forbidden)?;
        let agents = majority_collection(&self.database, AGENTS);
        let result = query_owned_agents(&agents, false, &caller.subject, query, page)
            .await
            .map_err(|error| match error {
                SandboxStoreError::InvalidPage => AgentReadError::InvalidPage,
                SandboxStoreError::Corrupt(reason) => AgentReadError::Corrupt(reason),
                _ => AgentReadError::Unavailable("agent inventory is unavailable".into()),
            })?;
        for agent in &result.items {
            let snapshot = self
                .personal_resource(
                    caller,
                    &ResourceTarget::Agent {
                        agent_id: agent.id.clone(),
                    },
                )
                .await
                .map_err(|error| match error {
                    ResourceAccessStoreError::Corrupt => {
                        AgentReadError::Corrupt("agent inventory lineage is invalid".into())
                    }
                    ResourceAccessStoreError::Unavailable => {
                        AgentReadError::Unavailable("agent inventory is unavailable".into())
                    }
                    _ => AgentReadError::NotFound,
                })?;
            if agent.owner != caller.subject
                || !snapshot.lineage.agent.is_some_and(|current| {
                    current.created_at == agent.created_at
                        && current.sandbox_id == agent.sandbox_id
                        && current.workspace_id == agent.workspace_id
                        && current.worktree_id == agent.worktree_id
                })
            {
                return Err(AgentReadError::Corrupt(
                    "agent inventory lineage is invalid".into(),
                ));
            }
        }
        Ok(result)
    }
}
