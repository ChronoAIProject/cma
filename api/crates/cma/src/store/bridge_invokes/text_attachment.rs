//! Capture once before command admission, sharing the exact live invoke fence.
use layer2_core::bridge::{
    ingress::{CapturedAttachmentText, StoredInbox},
    invoke::{BridgeInvokeError, BridgeInvokeProof},
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use super::{MongoBridgeInvokeStore, MutationError};
use crate::store::{
    bridge_definitions::{decode_inbox, inbox::text_attachment::require_attachment},
    durability::majority_collection,
    sandbox_mongo::{
        commit_transaction, is_transient, record_bson, start_transaction, CommitOutcome,
    },
};

impl MongoBridgeInvokeStore {
    /// The private custody reader shares the current Bridge/grant/owner/resource
    /// transaction fence. Held proof data alone cannot reopen a revoked source.
    pub(crate) async fn authorize_text_attachment_read(
        &self,
        proof: &BridgeInvokeProof,
        inbox: &StoredInbox,
    ) -> Result<(), BridgeInvokeError> {
        require_attachment(inbox, proof).map_err(|_| BridgeInvokeError::Unavailable)?;
        let mut session = self
            .bridges
            .client
            .start_session()
            .await
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        start_transaction(&mut session)
            .await
            .map_err(|_| BridgeInvokeError::Unavailable)?;
        if let Err(error) = self.fence_transaction(proof, &mut session).await {
            let _ = session.abort_transaction().await;
            return Err(match error {
                MutationError::Invoke(error) => error,
                MutationError::Mongo(_) => BridgeInvokeError::Unavailable,
            });
        }
        require_attachment(inbox, proof).map_err(|_| BridgeInvokeError::Unavailable)?;
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            _ => Err(BridgeInvokeError::Unavailable),
        }
    }

    pub(crate) async fn capture_text_attachment(
        &self,
        proof: &BridgeInvokeProof,
        expected: &StoredInbox,
        captured: &CapturedAttachmentText,
    ) -> Result<StoredInbox, BridgeInvokeError> {
        for _ in 0..4 {
            require_attachment(expected, proof).map_err(|_| BridgeInvokeError::Unavailable)?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            let result = self
                .capture_text_transaction(proof, expected, captured, &mut session)
                .await;
            let inbox = match result {
                Ok(inbox) => inbox,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Mongo(error) if is_transient(&error) => continue,
                        MutationError::Mongo(_) => return Err(BridgeInvokeError::Unavailable),
                        MutationError::Invoke(error) => return Err(error),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(inbox),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Failed(_) | CommitOutcome::Unknown(_) => {
                    return Err(BridgeInvokeError::Unavailable)
                }
            }
        }
        Err(BridgeInvokeError::Unavailable)
    }

    async fn capture_text_transaction(
        &self,
        proof: &BridgeInvokeProof,
        expected: &StoredInbox,
        captured: &CapturedAttachmentText,
        session: &mut ClientSession,
    ) -> Result<StoredInbox, MutationError> {
        self.fence_transaction(proof, session).await?;
        let database = self.bridges.client.database(&self.grants.namespace().db);
        let inboxes = majority_collection::<Document>(&database, "bridge_callback_inbox");
        let document = inboxes
            .find_one(doc! {"_id": &expected.id})
            .session(&mut *session)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let mut inbox = decode_inbox(&document).map_err(|_| BridgeInvokeError::Corrupt)?;
        require_attachment(&inbox, proof).map_err(|_| BridgeInvokeError::Unavailable)?;
        let mut pending = inbox.clone();
        pending
            .text_attachment
            .as_mut()
            .ok_or(BridgeInvokeError::Corrupt)?
            .captured = None;
        if &pending != expected
            || expected
                .text_attachment
                .as_ref()
                .is_none_or(|m| m.captured.is_some())
        {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        // The first committed bytes win. Every later attempt returns that same
        // captured input, even if CDN content changes or commit acknowledgement
        // was lost. It can never rewrite an admitted command's source.
        if inbox
            .text_attachment
            .as_ref()
            .is_some_and(|m| m.captured.is_some())
        {
            return Ok(inbox);
        }
        if document.contains_key("command_correlation") {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        inbox
            .text_attachment
            .as_mut()
            .ok_or(BridgeInvokeError::Corrupt)?
            .captured = Some(captured.clone());
        require_attachment(&inbox, proof).map_err(|_| BridgeInvokeError::Corrupt)?;
        let changed = inboxes.update_one(document, doc! {"$set": {"record": record_bson(&inbox).map_err(|_| BridgeInvokeError::Corrupt)?}})
            .session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        proof.require_current()?;
        Ok(inbox)
    }
}
