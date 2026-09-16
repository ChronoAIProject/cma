//! The canonical shared acceptance must agree with the complete retained intent.
use super::*;
use layer2_core::bridge::channel_control::ChannelControlIntent;
use mongodb::{bson::Document, Database};

impl MongoCallbackInbox {
    /// Read-only counterpart used by fresh execution authority and the final
    /// ordered-writer check. Retained intent alone cannot establish acceptance.
    pub(in crate::store) async fn require_accepted_control(
        database: &Database,
        intent: &ChannelControlIntent,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        accepted_control_documents(database, intent, session)
            .await
            .map(|_| ())
    }

    /// Persistence evidence only. Execution still needs separate fresh source,
    /// consent, allocation, runtime and control-operation authority.
    pub(in crate::store) async fn fence_accepted_control(
        database: &Database,
        intent: &ChannelControlIntent,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let rows = crate::store::durability::majority_collection::<Document>(
            database,
            "bridge_callback_dispositions",
        );
        for document in accepted_control_documents(database, intent, session).await? {
            let changed = rows
                .update_one(
                    doc! {"_id":document.get("_id").cloned().ok_or(CallbackError::Unavailable)?,
                    "record":document.get("record").cloned().ok_or(CallbackError::Unavailable)?},
                    doc! {"$inc":{"control_execution_fence":1_i64}},
                )
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(CallbackError::Unavailable.into());
            }
        }
        Ok(())
    }
}

async fn accepted_control_documents(
    database: &Database,
    intent: &ChannelControlIntent,
    session: &mut ClientSession,
) -> Result<[Document; 2], MutationError> {
    let rows = crate::store::durability::majority_collection::<Document>(
        database,
        "bridge_callback_dispositions",
    );
    let callback = rows
        .find_one(doc! {"_id":callback_id(&intent.callback.callback_id)})
        .session(&mut *session)
        .await?
        .ok_or(CallbackError::Unavailable)?;
    CallbackDisposition::read(&callback)?.require_control_intent(intent)?;
    let provider = rows
        .find_one(doc! {"_id":provider_id(&intent.callback.provider_key_sha256)})
        .session(&mut *session)
        .await?
        .ok_or(CallbackError::Unavailable)?;
    CallbackDisposition::read_provider(&provider, &intent.callback.provider_key_sha256)?
        .require_control_intent(intent)?;
    if callback.get("record") != provider.get("record") {
        return Err(CallbackError::ReplayConflict.into());
    }
    Ok([callback, provider])
}
