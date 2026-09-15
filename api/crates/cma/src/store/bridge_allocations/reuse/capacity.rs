use super::*;
use mongodb::bson::Bson;
use std::collections::BTreeSet;

impl MongoBridgeAllocationStore {
    pub(in crate::store::bridge_allocations) async fn require_reuse_capacity(
        &self,
        record: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let AllocationPlacement::ReuseParent { target, parent, .. } = &record.placement else {
            return Err(AllocationError::IdentityMismatch.into());
        };
        let reserve = record
            .config
            .placement
            .shared_capacity()
            .ok_or(AllocationError::IncompatibleTarget)?;
        let database = self.bridges.client.database(&self.records.namespace().db);
        let mut workspaces = BTreeSet::new();
        let mut agents = BTreeSet::new();
        let rows = majority_collection::<Document>(&database, "workspaces");
        let mut cursor = rows
            .find(doc! {"sandbox_id":target.sandbox_id.as_str(),"removed_at":Bson::Null})
            .limit(17)
            .session(&mut *session)
            .await?;
        while cursor.advance(&mut *session).await? {
            let row = cursor.deserialize_current()?;
            let workspace = crate::store::workspaces::read_workspace(&row)
                .map_err(|_| AllocationError::Corrupt)?;
            workspaces.insert(workspace.id);
        }
        let rows = majority_collection::<Document>(&database, "agents");
        let mut cursor = rows
            .find(doc! {"workspace_id":target.workspace_id.as_str(),"record.deleted_at":Bson::Null})
            .limit(17)
            .session(&mut *session)
            .await?;
        while cursor.advance(&mut *session).await? {
            let row = cursor.deserialize_current()?;
            let agent =
                crate::store::agents::read_agent(&row).map_err(|_| AllocationError::Corrupt)?;
            if agent.deleted_at.is_none() {
                agents.insert(agent.id);
            }
        }
        // Include reservations whose effects have not started. Retired records
        // remain capacity consumers until exact child cleanup is reconciled.
        let mut cursor = self
            .records
            .find(doc! {"record.placement.target.sandbox_id":target.sandbox_id.as_str()})
            .limit(257)
            .session(&mut *session)
            .await?;
        let mut seen = 0;
        while cursor.advance(&mut *session).await? {
            let row = cursor.deserialize_current()?;
            seen += 1;
            if seen > 256 {
                return Err(AllocationError::Capacity.into());
            }
            let retained = records::decode_record(&row)?;
            let agent_released = self
                .cleanup_released_agent_capacity(&retained, session)
                .await?;
            let workspace_released = self
                .cleanup_released_workspace_capacity(&retained, session)
                .await?;
            if let AllocationPlacement::ReuseParent {
                target: reserved,
                parent,
                ..
            } = retained.placement
            {
                if parent.workspace_created_at.is_none() && !workspace_released {
                    workspaces.insert(reserved.workspace_id.clone());
                }
                if reserved.workspace_id == target.workspace_id && !agent_released {
                    agents.insert(reserved.agent_id);
                }
            }
        }
        if parent.workspace_created_at.is_none() {
            workspaces.insert(target.workspace_id.clone());
        }
        agents.insert(target.agent_id.clone());
        if (parent.workspace_created_at.is_none()
            && workspaces.len() + usize::from(reserve.keep_other_workspaces)
                > usize::from(parent.limits.max_workspaces))
            || agents.len() + usize::from(reserve.keep_other_agents_per_workspace)
                > usize::from(parent.limits.max_agents_per_workspace)
        {
            return Err(AllocationError::Capacity.into());
        }
        Ok(())
    }
}
