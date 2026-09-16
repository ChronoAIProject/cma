//! Permanent pre-existing Message/refusal disposition wins before fresh work.
use super::*;

impl MongoCallbackInbox {
    pub(super) async fn prior_acceptance(
        &self,
        write: &InboxWrite,
        session: &mut ClientSession,
    ) -> Result<Option<CallbackAcceptance>, MutationError> {
        if let Some(document) = self
            .receipts
            .find_one(doc! {"_id": &write.record().id})
            .session(&mut *session)
            .await?
        {
            return Ok(Some(CallbackAcceptance::Message(
                receipts::model::AcceptedCallbackReceipt::read(&document)?.replay(write)?,
            )));
        }
        if let Some(document) = self
            .inbox
            .find_one(doc! {"_id": &write.record().id})
            .session(&mut *session)
            .await?
        {
            let source = decode(&document)?;
            let receipt = write.match_replay(&source)?;
            self.retain_acceptance(
                &source,
                receipts::model::command_correlation(&document)?,
                session,
            )
            .await?;
            return Ok(Some(CallbackAcceptance::Message(receipt)));
        }
        if let Some(receipt) = self
            .resolve_platform_redelivery(write, session, true)
            .await?
        {
            return Ok(Some(CallbackAcceptance::Message(receipt)));
        }
        if let Some(receipt) = self.resolve_disposition(write, session, true).await? {
            return Ok(Some(receipt));
        }
        Ok(None)
    }
}
