use layer2_core::bridge::allocation::AllocationReservation;

use super::*;

impl MongoBridgeInvokeStore {
    pub(in crate::store) async fn detach_allocation_usage(
        &self,
        write: &layer2_core::bridge::partition_commands::PartitionCommandWrite,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        write
            .require_current()
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        let previous = &write.snapshot().allocation;
        let (allocation, head) = self.usage_allocation(&previous.id, session).await?;
        if allocation != *previous {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let Some(head) = head else {
            return Ok(());
        };
        if head.linked_grants == 0 {
            return Ok(());
        }
        append_management_detach(
            &self.resources,
            &head.creation,
            write.management(),
            digest(&("cma.bridge.usage.partition-detach.v1", write.receipt_id()))?,
            &head.binding,
            ChannelBindingChangeReason::Detachment,
            session,
        )
        .await?;
        let mut next = head.clone();
        next.revision += 1;
        next.linked_grants = 0;
        self.write_usage_head(&allocation, Some(&head), &next, session)
            .await?;
        write
            .require_current()
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        Ok(())
    }
    pub(in crate::store) async fn reset_allocation_usage(
        &self,
        write: &AllocationReservation,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        write
            .require_current()
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        let previous = write.previous().ok_or(BridgeInvokeError::Corrupt)?;
        let management = write.reset_management().ok_or(BridgeInvokeError::Corrupt)?;
        let (allocation, head) = self.usage_allocation(&previous.id, session).await?;
        if allocation != *previous {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let Some(head) = head else {
            return Ok(());
        };
        if head.linked_grants == 0 {
            return Ok(());
        }
        append_management_detach(
            &self.resources,
            &head.creation,
            management,
            digest(&("cma.bridge.usage.reset.v1", &write.record().id))?,
            &head.binding,
            ChannelBindingChangeReason::Reset,
            session,
        )
        .await?;
        let mut next = head.clone();
        next.revision += 1;
        next.linked_grants = 0;
        self.write_usage_head(&allocation, Some(&head), &next, session)
            .await?;
        write
            .require_current()
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        Ok(())
    }
}
