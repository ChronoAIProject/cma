//! Exact management closure evidence permits real historical use; it never
//! grants invocation authority or restores an allocation's active association.
use super::management::UsageClosure;
use super::*;

impl MongoBridgeInvokeStore {
    pub(super) async fn usage_closure(
        &self,
        allocation: &AllocationRecord,
        grant: &BridgeInvokeGrant,
        session: &mut ClientSession,
    ) -> Result<Option<UsageClosure>, MutationError> {
        let document = self
            .usage_allocations()
            .find_one(doc! {"_id":&allocation.id})
            .session(&mut *session)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        if crate::store::bridge_allocations::decode_record(&document)
            .map_err(|_| BridgeInvokeError::Corrupt)?
            != *allocation
        {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let Some(closure) = UsageClosure::read(&document, allocation)? else {
            return Ok(None);
        };
        if grant.pin.bridge_id != allocation.identity.bridge_id
            || grant.owner != allocation.owner
            || grant.pin.config_revision > closure.invalid_through_config_revision
        {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        let database = self.bridges.client.database(&self.grants.namespace().db);
        let document = majority_collection::<Document>(&database, "bridge_definition_receipts")
            .find_one(doc! {"_id":&closure.management.id})
            .session(session)
            .await?
            .ok_or(BridgeInvokeError::Corrupt)?;
        let stored: layer2_core::bridge::StoredBridgeReceipt =
            read_record(&document).map_err(|_| BridgeInvokeError::Corrupt)?;
        if document.get_str("_id").ok() != Some(stored.id.as_str()) || stored != closure.management
        {
            return Err(BridgeInvokeError::Corrupt.into());
        }
        Ok(Some(closure))
    }

    pub(super) async fn historical_usage_link(
        &self,
        grant: &BridgeInvokeGrant,
        allocation: &AllocationRecord,
        previous: Option<&BindingHead>,
        existing_link: Option<&GrantLink>,
        closure: &UsageClosure,
        session: &mut ClientSession,
    ) -> Result<ChannelBindingReference, MutationError> {
        let snapshot = agent_channel_usage::read_inventory_snapshot(
            &self.resources,
            &grant.target.ids.agent_id,
            session,
        )
        .await?;
        grant
            .target
            .require(&grant.owner, &snapshot.provenance.resource)?;
        let creation = snapshot.state.creation.clone();
        if let Some(link) = existing_link {
            if !link.active {
                return Err(BridgeInvokeError::Conflict.into());
            }
            if link.allocation_id == allocation.id {
                link.require_head(previous.ok_or(BridgeInvokeError::Corrupt)?)?;
            } else {
                let (outgoing, head) = self.usage_allocation(&link.allocation_id, session).await?;
                let head = head.ok_or(BridgeInvokeError::Corrupt)?;
                link.require_head(&head)?;
                if outgoing.state != AllocationState::Retired
                    || head.linked_grants != 0
                    || self
                        .previous_usage_binding(allocation, session)
                        .await?
                        .as_ref()
                        != Some(&link.binding)
                {
                    return Err(BridgeInvokeError::TargetMismatch.into());
                }
            }
        }
        let historical_id = format!(
            "bruse_{}",
            digest(&(
                "cma.bridge.usage.historical.v1",
                &allocation.id,
                &closure.management.id
            ))?
        );
        let historical_binding = if closure.binding.is_none() {
            let predecessor = self.previous_usage_binding(allocation, session).await?;
            Some(ChannelBindingReference {
                binding_id: historical_id,
                bridge_id: allocation.identity.bridge_id.as_str().into(),
                partition_id: allocation.identity.key.hash().into(),
                generation: predecessor
                    .as_ref()
                    .map_or(1, |binding| binding.generation + 1),
                previous_binding_id: predecessor.map(|binding| binding.binding_id),
            })
        } else {
            None
        };
        let expected_binding = closure
            .binding
            .as_ref()
            .or(historical_binding.as_ref())
            .ok_or(BridgeInvokeError::Corrupt)?;
        let next = if let Some(head) = previous {
            if head.creation != creation
                || head.linked_grants != 0
                || expected_binding != &head.binding
            {
                return Err(BridgeInvokeError::TargetMismatch.into());
            }
            head.clone()
        } else {
            if closure.binding.is_some() {
                return Err(BridgeInvokeError::Corrupt.into());
            }
            let head = BindingHead {
                schema_version: 1,
                revision: 1,
                allocation_id: allocation.id.clone(),
                creation,
                binding: expected_binding.clone(),
                linked_grants: 0,
            };
            self.write_usage_head(allocation, None, &head, session)
                .await?;
            head
        };
        if snapshot
            .state
            .active_bindings
            .iter()
            .any(|active| active.reference == next.binding)
        {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let link = GrantLink::new(grant, &next);
        if existing_link != Some(&link) {
            let expected = match existing_link {
                Some(link) => record_bson(link).map_err(|_| BridgeInvokeError::Corrupt)?,
                None => mongodb::bson::Bson::Document(doc! {"$exists":false}),
            };
            let changed = self.grants.update_one(
                doc! {"_id":grant.id.as_str(),"revision":grant.revision as i64,"channel_usage_binding":expected},
                doc! {"$set":{"channel_usage_binding":record_bson(&link).map_err(|_|BridgeInvokeError::Corrupt)?}},
            ).session(session).await?;
            if changed.matched_count != 1 {
                return Err(BridgeInvokeError::Conflict.into());
            }
        }
        Ok(next.binding)
    }
}
