//! Shared current custody, replay, reply evidence and rate admission gates.
use super::*;
use crate::store::bridge_credentials::callback::CallbackBindingError;
use layer2_core::bridge::BridgeRecord;

impl MongoCallbackInbox {
    /// The dedicated control store owns recovery through this transaction.
    /// Fresh fallback may retain only an existing disabled/partition refusal;
    /// it cannot create accepted ordinary Message or control work.
    pub(in crate::store) async fn refuse_fresh_control(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
    ) -> Result<CallbackAcceptance, MutationError> {
        if write.record().admission == InboxAdmission::Accepted
            && write.control_refusal()
                != Some(
                    layer2_core::bridge::ingress::ChannelControlRefusal::ChannelControlsDisabled,
                )
        {
            return Err(CallbackError::Payload.into());
        }
        self.transaction(write, session).await
    }

    pub(in crate::store) fn require_control_store(
        &self,
        bridges: &MongoBridgeStore,
    ) -> Result<(), CallbackError> {
        if !Arc::ptr_eq(&self.bridges.credentials, &bridges.credentials)
            || self.inbox.namespace().db != bridges.definitions.namespace().db
        {
            return Err(CallbackError::Unavailable);
        }
        Ok(())
    }

    pub(in crate::store) async fn prepare_control(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
    ) -> Result<Option<CallbackAcceptance>, MutationError> {
        let current = self.require_callback_binding(write, session).await?;
        if let Some(receipt) = self.prior_acceptance(write, session).await? {
            return Ok(Some(receipt));
        }
        self.prepare_fresh(write, &current, session).await?;
        Ok(None)
    }

    pub(in crate::store) async fn recover_control_disposition(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
    ) -> Result<Option<CallbackAcceptance>, MutationError> {
        self.require_callback_binding(write, session).await?;
        self.prior_acceptance(write, session).await
    }

    pub(super) async fn require_callback_binding(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
    ) -> Result<BridgeRecord, MutationError> {
        write.require_current(write.bridge())?;
        let current = self
            .bridges
            .definitions
            .find_one(doc! {"_id":write.bridge().id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(CallbackError::BindingUnavailable)?;
        let current = decode_bridge(&current).map_err(|_| CallbackError::BindingUnavailable)?;
        write.require_current(&current)?;
        self.bridges
            .credentials
            .require_callback_binding(&current, write.custody_pin(), &mut *session)
            .await
            .map_err(binding_error)?;
        Ok(current)
    }

    pub(super) async fn prepare_fresh(
        &self,
        write: &InboxWrite,
        current: &BridgeRecord,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let record = write.record();
        // A transport-only verifier can construct InboxWrite before trusted raw
        // identity/text classification. It may recover history above, but cannot
        // bypass the reserved-control guard to create fresh Message work.
        if record.capabilities.is_none() {
            return Err(CallbackError::Payload.into());
        }
        self.require_confirmed_reply(record, session).await?;
        let config = &current
            .current()
            .map_err(|_| CallbackError::BindingUnavailable)?
            .config;
        if record.admission == InboxAdmission::Accepted && write.control_refusal().is_none() {
            let queued = self
                .inbox
                .count_documents(
                    doc! {"bridge_id":current.id.as_str(),"admission":"accepted", "command_correlation":{"$exists":false},
                    "expires_at":{"$gt":DateTime::from_millis(record.accepted_at * 1000)}},
                )
                .session(&mut *session)
                .await?;
            if queued >= u64::from(config.limits.max_queued_messages) {
                return Err(CallbackError::Capacity.into());
            }
        }
        let minute = record.accepted_at / 60;
        let rate_id = format!("{}:{minute}", current.id.as_str());
        let used = self
            .rates
            .find_one(doc! {"_id":&rate_id})
            .session(&mut *session)
            .await?
            .map(|d| d.get_i64("count").map_err(|_| CallbackError::Unavailable))
            .transpose()?
            .unwrap_or(0);
        if used < 0 || used >= i64::from(config.limits.max_messages_per_minute) {
            return Err(CallbackError::Capacity.into());
        }
        // A real write to the definition document serializes concurrent admissions
        // and races with disable, emergency stop, rotation or retirement. This
        // internal fence does not alter the user-visible configuration revision.
        let changed = self
            .bridges
            .definitions
            .update_one(
                doc! {"_id":current.id.as_str(),"revision":current.revision as i64},
                doc! {"$inc":{"inbox_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(CallbackError::BindingUnavailable.into());
        }
        self.rates.update_one(doc!{"_id":&rate_id},doc!{"$inc":{"count":1_i64},"$setOnInsert":{"document_schema_version":DOCUMENT_SCHEMA_VERSION,"expires_at":DateTime::from_millis((minute+2)*60000)}})
            .upsert(true).session(&mut *session).await?;
        Ok(())
    }
}

fn binding_error(error: CallbackBindingError) -> MutationError {
    match error {
        CallbackBindingError::Core(_) => CallbackError::BindingUnavailable.into(),
        CallbackBindingError::Driver(error) => MutationError::Mongo(error),
    }
}
