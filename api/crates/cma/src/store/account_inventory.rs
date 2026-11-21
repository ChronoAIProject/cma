use layer1_public::rest::PersonalAgentInventory;
use layer2_core::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStoreError, ResourceOwner, ResourceTarget,
};
use layer2_core::sandbox::store::{Page, PageRequest, SandboxStoreError};
use layer2_core::sandbox::{Agent, AgentListQuery, AgentReadError};
use layer2_core::{AgentId, Caller, Capability, CoreError, LaunchId, SubjectId};
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;

use super::MongoAccountStores;
use crate::store::{
    agent_commands::read_command,
    agent_log_integrity::TransactionLogError,
    agent_log_segments::retained_turn,
    agent_reads::query_owned_agents,
    durability::majority_collection,
    resource_access::{
        snapshot_lineage::{read_from, LineageCollections},
        ReadError,
    },
    sandbox_mongo::{AGENTS, AGENT_COMMANDS, RESOURCE_POLICIES, SANDBOXES, WORKSPACES, WORKTREES},
    snapshot_collection,
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
        self.personal_resource_in_session(caller, target, &mut self.snapshot_session().await?)
            .await
    }

    pub(crate) async fn authorize_personal_command(
        &self,
        caller: &Caller,
        selector: Document,
        agent: Option<&str>,
    ) -> Result<(), ResourceAccessStoreError> {
        // Resolve the ledger receipt and current owner at one snapshot time.
        // This path needs neither a command service nor an observation lease.
        let mut session = self.snapshot_session().await?;
        let commands = majority_collection(&self.database, AGENT_COMMANDS);
        let row = snapshot_collection::collection(&commands, true)
            .find_one(selector)
            .session(&mut session)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?
            .ok_or(ResourceAccessStoreError::NotFound)?;
        let command = read_command(&row).map_err(|_| ResourceAccessStoreError::Corrupt)?;
        let envelope = command.receipt.envelope;
        if agent.is_some_and(|id| id != envelope.agent_id.as_str()) {
            return Err(ResourceAccessStoreError::NotFound);
        }
        let target = ResourceTarget::Agent {
            agent_id: envelope.agent_id,
        };
        let snapshot = self
            .personal_resource_in_session(caller, &target, &mut session)
            .await?;
        if snapshot.scope(&target)?.owner != envelope.owner {
            return Err(ResourceAccessStoreError::NotFound);
        }
        Ok(())
    }

    pub(crate) async fn authorize_personal_turn(
        &self,
        caller: &Caller,
        agent_id: AgentId,
        turn: u64,
    ) -> Result<(), ResourceAccessStoreError> {
        let mut session = self.snapshot_session().await?;
        let target = ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        };
        self.personal_resource_in_session(caller, &target, &mut session)
            .await?;
        let agents = majority_collection(&self.database, AGENTS);
        let row = snapshot_collection::collection(&agents, true)
            .find_one(doc! {"_id": agent_id.as_str()})
            .session(&mut session)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?
            .ok_or(ResourceAccessStoreError::NotFound)?;
        let exists = retained_turn(&self.database, &row, turn, &mut session)
            .await
            .map_err(|error| match error {
                TransactionLogError::Driver(_) => ResourceAccessStoreError::Unavailable,
                TransactionLogError::Domain(_) => ResourceAccessStoreError::Corrupt,
            })?;
        exists
            .then_some(())
            .ok_or(ResourceAccessStoreError::NotFound)
    }

    async fn snapshot_session(&self) -> Result<ClientSession, ResourceAccessStoreError> {
        self.database
            .client()
            .start_session()
            .snapshot(true)
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)
    }

    async fn personal_resource_in_session(
        &self,
        caller: &Caller,
        target: &ResourceTarget,
        session: &mut ClientSession,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        let (lineage, policy) = read_from(
            LineageCollections {
                agents: &majority_collection(&self.database, AGENTS),
                worktrees: &majority_collection(&self.database, WORKTREES),
                workspaces: &majority_collection(&self.database, WORKSPACES),
                sandboxes: &majority_collection(&self.database, SANDBOXES),
                resource_policies: &majority_collection(&self.database, RESOURCE_POLICIES),
            },
            target,
            session,
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
