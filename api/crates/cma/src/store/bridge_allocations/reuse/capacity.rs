use super::*;
use mongodb::bson::Bson;
use std::collections::BTreeSet;

impl MongoBridgeAllocationStore {
    pub(in crate::store::bridge_allocations) async fn require_reuse_capacity(
        &self,
        record: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        self.reuse_capacity(record, false, session).await
    }

    pub(in crate::store::bridge_allocations) async fn require_reuse_continuation_capacity(
        &self,
        record: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        self.reuse_capacity(record, true, session).await
    }

    async fn reuse_capacity(
        &self,
        record: &AllocationRecord,
        continuing: bool,
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
        let mut retained_original = false;
        for (retained, agent_released, workspace_released) in
            Self::retained_reuse_reservations(&database, &target.sandbox_id, session).await?
        {
            if retained.id == record.id {
                if retained != *record
                    || agent_released
                    || (parent.workspace_created_at.is_none() && workspace_released)
                {
                    return Err(AllocationError::Corrupt.into());
                }
                retained_original = true;
            }
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
        // An existing exact reservation has already kept the owner's spare.
        // Ordinary creation may now occupy it. Continue the original IDs only
        // while they remain fully accounted and the physical limit still fits.
        if continuing && !retained_original {
            return Err(AllocationError::Corrupt.into());
        }
        if parent.workspace_created_at.is_none() {
            workspaces.insert(target.workspace_id.clone());
        }
        agents.insert(target.agent_id.clone());
        let spare_workspaces = if continuing {
            0
        } else {
            usize::from(reserve.keep_other_workspaces)
        };
        let spare_agents = if continuing {
            0
        } else {
            usize::from(reserve.keep_other_agents_per_workspace)
        };
        if (parent.workspace_created_at.is_none()
            && workspaces.len() + spare_workspaces > usize::from(parent.limits.max_workspaces))
            || agents.len() + spare_agents > usize::from(parent.limits.max_agents_per_workspace)
        {
            return Err(AllocationError::Capacity.into());
        }
        Ok(())
    }

    /// Called only inside ordinary or Channel admission's parent write fence.
    /// Include every Bridge and retained generation; only exact existing cleanup
    /// proofs may release a reservation. A truncated inventory grants no slot.
    pub(in crate::store) async fn retained_reuse_reservations(
        database: &mongodb::Database,
        sandbox_id: &layer2_core::SandboxId,
        session: &mut ClientSession,
    ) -> Result<Vec<(AllocationRecord, bool, bool)>, MutationError> {
        let records = majority_collection::<Document>(database, "bridge_allocations");
        let mut cursor = records
            .find(doc! {"record.placement.target.sandbox_id":sandbox_id.as_str()})
            .limit(257)
            .session(&mut *session)
            .await?;
        let mut reservations = Vec::new();
        let mut seen = 0;
        while cursor.advance(&mut *session).await? {
            seen += 1;
            if seen > 256 {
                return Err(AllocationError::Capacity.into());
            }
            let record = records::decode_record(&cursor.deserialize_current()?)?;
            if !matches!(record.placement, AllocationPlacement::ReuseParent { .. }) {
                continue;
            }
            let agent_released =
                Self::released_reuse_agent_capacity(database, &record, session).await?;
            let workspace_released =
                Self::released_reuse_workspace_capacity(database, &record, session).await?;
            reservations.push((record, agent_released, workspace_released));
        }
        Ok(reservations)
    }
}
