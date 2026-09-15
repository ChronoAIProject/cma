use crate::store::sandbox_mongo::record_bson;
use layer2_core::sandbox::agent_io::{ChannelCommandAdmissionError, ChannelCommandReservation};
use mongodb::{bson::doc, ClientSession};

use super::{MongoChannelCommandStore, MutationError};

impl MongoChannelCommandStore {
    pub(super) async fn bind_sidecar(
        &self,
        request: &ChannelCommandReservation,
        created: bool,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let id = request.reservation().envelope.command_id.as_str();
        let document = self
            .invokes
            .resources
            .agent_commands
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?
            .ok_or(ChannelCommandAdmissionError::SourceMismatch)?;
        let mut expected_sidecar = request.sidecar()?;
        if !created {
            let retained: layer2_core::sandbox::agent_io::ChannelCommandSidecar =
                mongodb::bson::deserialize_from_bson(
                    document
                        .get("channel_admission")
                        .cloned()
                        .ok_or(ChannelCommandAdmissionError::SourceMismatch)?,
                )
                .map_err(|_| ChannelCommandAdmissionError::SourceMismatch)?;
            expected_sidecar.execution_hold = retained.execution_hold;
        }
        let expected = record_bson(&expected_sidecar)
            .map_err(|_| ChannelCommandAdmissionError::SourceMismatch)?;
        let archived = super::super::channel_source::archive(request.inbox(), &expected_sidecar)?;
        if !created {
            if document.get("channel_admission") != Some(&expected) {
                return Err(ChannelCommandAdmissionError::SourceMismatch.into());
            }
            if let Some(source) = document.get("channel_source") {
                if source != &archived {
                    return Err(ChannelCommandAdmissionError::SourceMismatch.into());
                }
                return Ok(());
            }
        }
        let changed = self
            .invokes
            .resources
            .agent_commands
            .update_one(
                doc! {"_id":id,"channel_source":{"$exists":false}},
                doc! {"$set":{"channel_admission":expected,"channel_source":archived}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(ChannelCommandAdmissionError::SourceMismatch.into());
        }
        Ok(())
    }

    pub(super) async fn fence_retained(
        &self,
        request: &ChannelCommandReservation,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let inbox_document = self
            .inbox
            .find_one(doc! {"_id": &request.inbox().id})
            .session(&mut *session)
            .await?
            .ok_or(ChannelCommandAdmissionError::SourceMismatch)?;
        let inbox = crate::store::bridge_definitions::decode_inbox(&inbox_document)
            .map_err(|_| ChannelCommandAdmissionError::SourceMismatch)?;
        let allocation_document = self
            .allocations
            .find_one(doc! {"_id": &request.allocation().id})
            .session(&mut *session)
            .await?
            .ok_or(ChannelCommandAdmissionError::SourceMismatch)?;
        let allocation = crate::store::bridge_allocations::decode_record(&allocation_document)
            .map_err(|_| ChannelCommandAdmissionError::SourceMismatch)?;
        request.require_records(&inbox, &allocation)?;
        let reservation = request.reservation();
        let correlation = doc! {
            "command_id": reservation.envelope.command_id.as_str(),
            "response_id": reservation.response_id.as_ref()
                .ok_or(ChannelCommandAdmissionError::SourceMismatch)?.as_str(),
            "allocation_id": &allocation.id,
            "body_sha256": &inbox.attestation.body_sha256,
        };
        if inbox_document
            .get("command_correlation")
            .is_some_and(|stored| stored.as_document() != Some(&correlation))
        {
            return Err(ChannelCommandAdmissionError::SourceMismatch.into());
        }
        // Both real writes participate in the command transaction, including
        // replay. Reset/retirement, message removal and competing correlation
        // writes must conflict instead of validating only a stale read snapshot.
        let changed = self
            .inbox
            .update_one(
                doc! {"_id": &inbox.id},
                doc! {"$inc": {"command_admission_fence": 1_i64},
                "$set": {"command_correlation": correlation}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(ChannelCommandAdmissionError::SourceMismatch.into());
        }
        let changed = self
            .allocations
            .update_one(
                doc! {"_id": &allocation.id, "revision": allocation.revision as i64},
                doc! {"$inc": {"command_admission_fence": 1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(ChannelCommandAdmissionError::SourceMismatch.into());
        }
        request.require_current()?;
        Ok(())
    }
}
