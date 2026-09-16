use super::*;

impl MongoBridgeInvokeStore {
    pub(super) async fn attach_grant_usage(
        &self,
        write: &BridgeInvokeWrite,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        write.require_current()?;
        // Fixed-Agent consent can precede the real partition allocation. Its
        // first signed common admission supplies that actual association.
        let Some(basis) = &write.record().channel else {
            return Ok(());
        };
        if self
            .usage_closure(&basis.allocation, write.record(), session)
            .await?
            .is_some()
        {
            // An old Channel-created grant can still be issued under its real
            // DrainOriginal proof. Its first accepted common command records
            // historical use; grant issue alone creates no historical binding.
            write.require_current()?;
            return Ok(());
        }
        self.ensure_usage_link(
            write.record(),
            &basis.allocation,
            FencedUsageActor::Management(write.management()),
            &write.receipt().id,
            session,
        )
        .await?;
        write.require_current()?;
        Ok(())
    }

    pub(super) async fn ensure_usage_link(
        &self,
        grant: &BridgeInvokeGrant,
        expected_allocation: &AllocationRecord,
        actor: FencedUsageActor<'_>,
        action_id: &str,
        session: &mut ClientSession,
    ) -> Result<(ChannelBindingReference, bool), MutationError> {
        grant.require_active()?;
        let (allocation, previous) = self
            .usage_allocation(&expected_allocation.id, session)
            .await?;
        if allocation != *expected_allocation
            || allocation.state != AllocationState::Ready
            || allocation.placement.target() != &grant.target.ids
            || allocation.owner != grant.owner
        {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        let grant_document = self
            .grants
            .find_one(doc! {"_id":grant.id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        if read_grant(&grant_document)? != *grant {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let existing_link = GrantLink::read(&grant_document, grant)?;
        if let Some(closure) = self.usage_closure(&allocation, grant, session).await? {
            let binding = self
                .historical_usage_link(
                    grant,
                    &allocation,
                    previous.as_ref(),
                    existing_link.as_ref(),
                    &closure,
                    session,
                )
                .await?;
            return Ok((binding, true));
        }
        if let Some(link) = &existing_link {
            if link.allocation_id == allocation.id {
                let head = previous.as_ref().ok_or(BridgeInvokeError::Corrupt)?;
                link.require_head(head)?;
                if !link.active || head.linked_grants == 0 {
                    return Err(BridgeInvokeError::Conflict.into());
                }
                return Ok((link.binding.clone(), false));
            }
            let (outgoing, outgoing_head) =
                self.usage_allocation(&link.allocation_id, session).await?;
            let outgoing_head = outgoing_head.ok_or(BridgeInvokeError::Corrupt)?;
            link.require_head(&outgoing_head)?;
            if outgoing.state != AllocationState::Retired
                || outgoing_head.linked_grants != 0
                || self
                    .previous_usage_binding(&allocation, session)
                    .await?
                    .as_ref()
                    != Some(&link.binding)
            {
                return Err(BridgeInvokeError::TargetMismatch.into());
            }
        }
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
        let next = match &previous {
            Some(head) if head.linked_grants > 0 => {
                if head.creation != creation
                    || snapshot
                        .state
                        .active_bindings
                        .iter()
                        .all(|active| active.reference != head.binding)
                {
                    return Err(BridgeInvokeError::TargetMismatch.into());
                }
                let mut next = head.clone();
                next.revision += 1;
                next.linked_grants += 1;
                next
            }
            _ => {
                let predecessor = match &previous {
                    Some(head) => Some(head.binding.clone()),
                    None => self.previous_usage_binding(&allocation, session).await?,
                };
                let binding = ChannelBindingReference {
                    binding_id: format!(
                        "bruse_{}",
                        digest(&("cma.bridge.usage.binding.v1", &allocation.id, action_id,))?
                    ),
                    bridge_id: allocation.identity.bridge_id.as_str().into(),
                    partition_id: allocation.identity.key.hash().into(),
                    generation: predecessor
                        .as_ref()
                        .map_or(1, |binding| binding.generation + 1),
                    previous_binding_id: predecessor.map(|binding| binding.binding_id),
                };
                append_fenced_change(
                    &self.resources,
                    &creation.agent_id,
                    actor,
                    digest(&("cma.bridge.usage.attach.v1", &binding.binding_id))?,
                    ChannelUsageChange::Attach {
                        binding: binding.clone(),
                    },
                    ChannelBindingChangeReason::Attachment,
                    session,
                )
                .await?;
                BindingHead {
                    schema_version: 1,
                    revision: previous.as_ref().map_or(1, |head| head.revision + 1),
                    allocation_id: allocation.id.clone(),
                    creation,
                    binding,
                    linked_grants: 1,
                }
            }
        };
        self.write_usage_head(&allocation, previous.as_ref(), &next, session)
            .await?;
        let link = GrantLink::new(grant, &next);
        let expected_link = match existing_link {
            Some(link) => record_bson(&link).map_err(|_| BridgeInvokeError::Corrupt)?,
            None => mongodb::bson::Bson::Document(doc! {"$exists":false}),
        };
        let changed = self.grants.update_one(
            doc! {"_id":grant.id.as_str(),"revision":grant.revision as i64,"channel_usage_binding":expected_link},
            doc! {"$set":{"channel_usage_binding":record_bson(&link).map_err(|_|BridgeInvokeError::Corrupt)?}},
        ).session(session).await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        Ok((next.binding, false))
    }

    pub(super) async fn previous_usage_binding(
        &self,
        allocation: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<Option<ChannelBindingReference>, MutationError> {
        let mut current = allocation.clone();
        // Allocation generations already have this bound. Empty failed
        // generations cannot invent a usage predecessor that never existed.
        for _ in 0..256 {
            let Some(predecessor) = &current.predecessor else {
                return Ok(None);
            };
            let (previous, head) = self.usage_allocation(&predecessor.id, session).await?;
            if previous.identity.bridge_id != current.identity.bridge_id
                || previous.identity.key != current.identity.key
                || previous.identity.generation + 1 != current.identity.generation
                || previous.owner != current.owner
                || previous.state != AllocationState::Retired
            {
                return Err(BridgeInvokeError::TargetMismatch.into());
            }
            if let Some(head) = head {
                if head.linked_grants != 0 {
                    return Err(BridgeInvokeError::Conflict.into());
                }
                return Ok(Some(head.binding));
            }
            current = previous;
        }
        Err(BridgeInvokeError::Corrupt.into())
    }
}
