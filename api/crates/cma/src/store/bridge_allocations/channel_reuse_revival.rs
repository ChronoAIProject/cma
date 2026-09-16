//! Reuse revival authorizes only the already-created child and its ordinary
//! parent. It never reopens provisioning or acquires parent credential custody.
use super::*;
use crate::{
    bridge::provisioning::reuse_revival::ChannelReuseRevivalAuthority,
    sandbox::{
        channel_provisioning::state::ChannelProvisioningPhase,
        channel_reuse::state::{ChannelReuseSnapshot, ChannelReuseState},
    },
    store::sandbox_mongo::record_bson,
};
use layer2_core::sandbox::{WorkspaceStatus, WorktreeStatus};
use mongodb::Database;

mod head;

impl MongoBridgeAllocationStore {
    pub(crate) async fn fence_channel_reuse_revival(
        &self,
        permit: &ChannelReuseRevivalAuthority,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        permit.require_current()?;
        let authority = permit.allocation_authority();
        let allocation = permit.allocation();
        let database = self.bridges.client.database(&self.records.namespace().db);
        if self.bridges.definitions.namespace().db != database.name()
            || self.heads.namespace().db != database.name()
            || self
                .invokes
                .as_ref()
                .is_none_or(|invokes| invokes.resources.sandboxes.namespace().db != database.name())
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        // Reuse's ordinary-parent proof fences exact current ManageSandbox
        // policy and lineage, including newly inserted policy/share records.
        self.fence(authority, session).await?;
        self.bridges
            .credentials
            .require_control_plane_binding(authority.bridge(), permit.human(), session)
            .await
            .map_err(|error| match error {
                crate::store::bridge_credentials::control_plane::ControlPlaneFenceError::Core(
                    _,
                ) => MutationError::Core(AllocationError::AuthorityUnavailable),
                crate::store::bridge_credentials::control_plane::ControlPlaneFenceError::Driver(
                    error,
                ) => MutationError::Mongo(error),
            })?;
        let row = self
            .records
            .find_one(doc! {"_id":&allocation.id})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Conflict)?;
        if &records::decode_record(&row)? != allocation || row.contains_key("sandbox_admission") {
            return Err(AllocationError::IdentityMismatch.into());
        }
        let state: ChannelReuseState = decode(&row, "channel_reuse")?;
        state.validate(allocation)?;
        if state.phase != ChannelProvisioningPhase::Ready {
            return Err(AllocationError::AuthorityUnavailable.into());
        }
        let target = allocation.placement.target();
        let sandbox_row = read(&database, "sandboxes", target.sandbox_id.as_str(), session).await?;
        let sandbox = crate::store::sandboxes::read_sandbox(&sandbox_row)
            .map_err(|_| AllocationError::Corrupt)?;
        if &sandbox != permit.sandbox()
            || sandbox_row.contains_key("channel_creation")
            || sandbox_row.contains_key("channel_provisioning")
            || sandbox_row.contains_key("channel_renewal")
            || sandbox_row.contains_key("channel_lifecycle_credential")
            || sandbox_row.contains_key("channel_cleanup")
            || sandbox_row.contains_key("channel_mint_reconciliation")
            || sandbox_row.contains_key("channel_retirement")
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        let workspace_row = read(
            &database,
            "workspaces",
            target.workspace_id.as_str(),
            session,
        )
        .await?;
        let tree_row = read(&database, "worktrees", target.worktree_id.as_str(), session).await?;
        let agent_row = read(&database, "agents", target.agent_id.as_str(), session).await?;
        let creation_row = read(
            &database,
            "agent_creation_provenance",
            target.agent_id.as_str(),
            session,
        )
        .await?;
        let workspace = crate::store::workspaces::read_workspace(&workspace_row)
            .map_err(|_| AllocationError::Corrupt)?;
        let worktree = crate::store::worktrees::read_worktree(&tree_row)
            .map_err(|_| AllocationError::Corrupt)?;
        let agent =
            crate::store::agents::read_agent(&agent_row).map_err(|_| AllocationError::Corrupt)?;
        if workspace.status != WorkspaceStatus::Ready
            || worktree.status != WorktreeStatus::Ready
            || worktree.agent_id.as_ref() != Some(&agent.id)
            || !agent.status.is_live()
            || agent.requested_stop_reason.is_some()
            || agent.conversation.conversation_id.is_none()
        {
            return Err(AllocationError::AuthorityUnavailable.into());
        }
        let snapshot = ChannelReuseSnapshot {
            allocation: allocation.clone(),
            sandbox,
            state,
            workspace: Some(workspace),
            worktree: Some(worktree),
            agent: Some(agent),
            creation: Some(
                crate::store::agent_provenance::read(&creation_row)
                    .map_err(|_| AllocationError::Corrupt)?,
            ),
            workspace_creation: workspace_row
                .get("channel_reuse_creation")
                .map(|value| {
                    mongodb::bson::deserialize_from_bson(value.clone())
                        .map_err(|_| AllocationError::Corrupt)
                })
                .transpose()?,
        };
        snapshot.require(authority)?;
        head::fence(self, allocation, session).await?;
        for (name, row) in [
            ("sandboxes", sandbox_row),
            ("workspaces", workspace_row),
            ("worktrees", tree_row),
            ("agents", agent_row),
            ("agent_creation_provenance", creation_row),
        ] {
            let rows: Collection<Document> = majority_collection(&database, name);
            let changed = rows
                .update_one(row, doc! {"$inc":{"channel_reuse_revival_fence":1_i64}})
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 || changed.modified_count != 1 {
                return Err(AllocationError::Conflict.into());
            }
        }
        let changed = self.records.update_one(
            doc! {"_id":&allocation.id,"revision":allocation.revision as i64,
                "channel_reuse":record_bson(&snapshot.state).map_err(|_| AllocationError::Corrupt)?},
            doc! {"$inc":{"channel_reuse_revival_fence":1_i64}},
        ).session(session).await?;
        if changed.matched_count != 1 || changed.modified_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        permit.require_current()?;
        Ok(())
    }
}

async fn read(
    database: &Database,
    name: &str,
    id: &str,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    let rows: Collection<Document> = majority_collection(database, name);
    rows.find_one(doc! {"_id":id})
        .session(session)
        .await?
        .ok_or(AllocationError::Conflict.into())
}

fn decode<T: serde::de::DeserializeOwned>(row: &Document, key: &str) -> Result<T, AllocationError> {
    mongodb::bson::deserialize_from_bson(row.get(key).cloned().ok_or(AllocationError::Corrupt)?)
        .map_err(|_| AllocationError::Corrupt)
}
