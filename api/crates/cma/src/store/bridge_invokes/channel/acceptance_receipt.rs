use layer2_core::bridge::ingress::StoredInbox;
use layer2_core::sandbox::agent_io::ChannelCommandAdmissionError as Error;
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use super::{MongoChannelCommandStore, MutationError};
use crate::store::bridge_definitions::inbox::receipts::model::AcceptedCallbackReceipt;

impl MongoChannelCommandStore {
    /// Bind the permanent receipt in the same transaction as command admission,
    /// its original source archive, and the inbox/allocation authority fences.
    pub(super) async fn bind_acceptance_receipt(
        &self,
        inbox: &StoredInbox,
        correlation: &Document,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let prior = self
            .acceptance_receipts
            .find_one(doc! {"_id": &inbox.id})
            .session(&mut *session)
            .await?;
        let mut receipt = match &prior {
            Some(document) => {
                let value =
                    AcceptedCallbackReceipt::read(document).map_err(|_| Error::SourceMismatch)?;
                value
                    .require_source(inbox)
                    .map_err(|_| Error::SourceMismatch)?;
                value
            }
            None => AcceptedCallbackReceipt::capture(inbox).map_err(|_| Error::SourceMismatch)?,
        };
        receipt
            .bind(correlation)
            .map_err(|_| Error::SourceMismatch)?;
        let document = receipt.document().map_err(|_| Error::SourceMismatch)?;
        match prior {
            Some(prior) if prior.get("record") != document.get("record") => {
                let changed = self.acceptance_receipts.replace_one(
                    doc! {"_id": &inbox.id,"record": prior.get("record").cloned().ok_or(Error::SourceMismatch)?},document,
                ).session(&mut *session).await?;
                if changed.matched_count != 1 {
                    return Err(Error::SourceMismatch.into());
                }
            }
            Some(_) => {}
            None => {
                self.acceptance_receipts
                    .insert_one(document)
                    .session(&mut *session)
                    .await?;
            }
        }
        Ok(())
    }
}
