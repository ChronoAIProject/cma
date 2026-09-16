use super::*;
use crate::{
    bridge::{allocation::AllocationPlacement, BridgeId},
    resource_access::{AccessPurpose, CallerContext, ResourceCapability, ResourceTarget},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

impl BridgeCleanupService {
    pub async fn allocations(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        bridge: &BridgeId,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<CleanupAllocationPage, CleanupError> {
        if !(1..=100).contains(&limit) {
            return Err(CleanupError::Invalid);
        }
        let after = cursor
            .map(|value| decode_cursor(bridge, value))
            .transpose()?;
        let after = after.as_deref();
        let mut manager = self.management(context, credential, bridge).await?;
        let records = self.store.allocations(&manager, after, limit).await?;
        if records.len() > limit as usize + 1 {
            return Err(CleanupError::Corrupt);
        }
        let mut previous = after;
        for record in &records {
            record.validate().map_err(|_| CleanupError::Corrupt)?;
            if record.identity.bridge_id != *bridge
                || record.owner != manager.record().owner
                || previous.is_some_and(|id| id >= record.id.as_str())
            {
                return Err(CleanupError::Corrupt);
            }
            previous = Some(&record.id);
        }
        let more = records.len() > limit as usize;
        let mut items = Vec::new();
        let mut parents = std::collections::BTreeMap::new();
        for record in records.iter().take(limit as usize) {
            manager.require_current()?;
            let sandbox_id = &record.placement.target().sandbox_id;
            if !parents.contains_key(sandbox_id) {
                let resource = match self
                    .resources
                    .authorize(
                        context,
                        credential,
                        &ResourceTarget::Sandbox {
                            sandbox_id: sandbox_id.clone(),
                        },
                        ResourceCapability::ManageSandbox,
                        AccessPurpose::Read,
                    )
                    .await
                    .map_err(CleanupError::from)
                {
                    Ok(resource) => resource,
                    Err(CleanupError::NotFound) => continue,
                    Err(error) => return Err(error),
                };
                resource
                    .decision
                    .require(ResourceCapability::ManageSandbox)
                    .map_err(CleanupError::from)?;
                if resource
                    .snapshot
                    .scope(&ResourceTarget::Sandbox {
                        sandbox_id: sandbox_id.clone(),
                    })
                    .map_err(|_| CleanupError::Corrupt)?
                    .owner
                    != manager.record().owner
                {
                    return Err(CleanupError::NotFound);
                }
                if let Some(deadline) = resource
                    .decision
                    .revalidation_deadline(ResourceCapability::ManageSandbox)
                {
                    manager.bound_deadline(deadline);
                }
                parents.insert(sandbox_id.clone(), resource);
            }
            items.push(CleanupAllocationView {
                id: record.id.clone(),
                bridge_id: bridge.clone(),
                generation: record.identity.generation,
                revision: record.revision,
                target: record.placement.target().clone(),
                state: record.state,
                placement: match record.placement {
                    AllocationPlacement::ExistingAgent { .. } => CleanupPlacement::ExistingAgent,
                    AllocationPlacement::NewSandbox { .. } => CleanupPlacement::NewSandbox,
                    AllocationPlacement::ReuseParent { .. } => CleanupPlacement::ReuseParent,
                },
            });
        }
        let next_cursor = more
            .then(|| encode_cursor(bridge, &records[limit as usize - 1].id))
            .transpose()?;
        for (sandbox_id, original) in parents {
            let current = self
                .resources
                .authorize(
                    context,
                    credential,
                    &ResourceTarget::Sandbox { sandbox_id },
                    ResourceCapability::ManageSandbox,
                    AccessPurpose::Read,
                )
                .await
                .map_err(CleanupError::from)?;
            if current.snapshot != original.snapshot {
                return Err(CleanupError::Conflict);
            }
            original
                .decision
                .require(ResourceCapability::ManageSandbox)
                .map_err(CleanupError::from)?;
            manager.require_current()?;
        }
        self.definitions.validate_management(&manager).await?;
        Ok(CleanupAllocationPage { items, next_cursor })
    }
}

fn encode_cursor(bridge: &BridgeId, after: &str) -> Result<String, CleanupError> {
    let bytes = serde_json::to_vec(&(1_u8, bridge, after)).map_err(|_| CleanupError::Corrupt)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn decode_cursor(bridge: &BridgeId, cursor: &str) -> Result<String, CleanupError> {
    if cursor.is_empty() || cursor.len() > 1024 {
        return Err(CleanupError::Invalid);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| CleanupError::Invalid)?;
    let (version, scope, after): (u8, BridgeId, String) =
        serde_json::from_slice(&bytes).map_err(|_| CleanupError::Invalid)?;
    if version != 1
        || scope != *bridge
        || !allocation_id(&after)
        || encode_cursor(bridge, &after)? != cursor
    {
        return Err(CleanupError::Invalid);
    }
    Ok(after)
}
