use super::*;

impl MongoBridgeInvokeStore {
    pub(super) async fn revoke_grant_usage(
        &self,
        write: &BridgeInvokeWrite,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        write.require_current()?;
        let grant = write.record();
        let document = self
            .grants
            .find_one(doc! {"_id":grant.id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        if read_grant(&document)? != *grant {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let Some(link) = GrantLink::read(&document, grant)? else {
            // Consent without a real allocation has never attached an Agent.
            return Ok(());
        };
        if !link.active {
            return Err(BridgeInvokeError::Corrupt.into());
        }
        let (allocation, head) = self.usage_allocation(&link.allocation_id, session).await?;
        let head = head.ok_or(BridgeInvokeError::Corrupt)?;
        link.require_head(&head)?;
        if head.linked_grants > 0 {
            let mut next = head.clone();
            next.revision += 1;
            next.linked_grants -= 1;
            if next.linked_grants == 0 {
                append_management_detach(
                    &self.resources,
                    &head.creation,
                    write.management(),
                    digest(&("cma.bridge.usage.revoke.v1", &write.receipt().id))?,
                    &head.binding,
                    ChannelBindingChangeReason::Detachment,
                    session,
                )
                .await?;
            }
            self.write_usage_head(&allocation, Some(&head), &next, session)
                .await?;
        }
        // A reset may already have detached this old association. Revocation
        // retains that original event and cannot decrement a newer binding.
        let mut next_link = link.clone();
        next_link.active = false;
        let changed = self.grants.update_one(
            doc! {"_id":grant.id.as_str(),"revision":grant.revision as i64,
                "channel_usage_binding":record_bson(&link).map_err(|_|BridgeInvokeError::Corrupt)?},
            doc! {"$set":{"channel_usage_binding":record_bson(&next_link).map_err(|_|BridgeInvokeError::Corrupt)?}},
        ).session(session).await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        write.require_current()?;
        Ok(())
    }
}
