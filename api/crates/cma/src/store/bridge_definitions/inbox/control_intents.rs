//! Optional disabled-intent evidence, committed with the truthful old refusal.
//! No production callback handler calls this foundation. These rows can never
//! become executable intents, even after future source-control enablement.
use layer2_core::bridge::channel_control::{
    ChannelControlCallback, ChannelControlWrite, RefusedChannelControlIntent,
};
use serde::{Deserialize, Serialize};

use super::*;

pub(crate) struct RefusedControlAcceptance {
    pub acceptance: CallbackAcceptance,
    pub intent: Option<RefusedChannelControlIntent>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Alias {
    schema_version: u8,
    callback: ChannelControlCallback,
    intent_id: String,
    intent_sha256: String,
}

impl MongoCallbackInbox {
    pub(crate) async fn retain_refused_control(
        &self,
        write: ChannelControlWrite,
    ) -> Result<RefusedControlAcceptance, CallbackError> {
        let candidate =
            RefusedChannelControlIntent::capture(&write).map_err(|_| CallbackError::Payload)?;
        for _ in 0..8 {
            write.inbox().require_current(write.inbox().bridge())?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| CallbackError::Unavailable)?;
            let result = match self
                .refused_control_transaction(&write, &candidate, &mut session)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Mongo(error)
                            if is_duplicate_key(&error) || is_transient(&error) =>
                        {
                            continue
                        }
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(_) => return Err(CallbackError::Unavailable),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return self.recover_refused_control(&write).await;
                }
            }
        }
        Err(CallbackError::Unavailable)
    }

    async fn refused_control_transaction(
        &self,
        write: &ChannelControlWrite,
        candidate: &RefusedChannelControlIntent,
        session: &mut ClientSession,
    ) -> Result<RefusedControlAcceptance, MutationError> {
        // This first read decides whether new optional evidence is allowed.
        // The established transaction below still verifies current binding,
        // custody, capacity, rate and exact selected-bot reply facts.
        let prior = self.prior_acceptance(write.inbox(), session).await?;
        let acceptance = self.transaction(write.inbox(), session).await?;
        let intents = self.control_collection("bridge_refused_control_intents");
        let aliases = self.control_collection("bridge_refused_control_aliases");
        let retained = intents
            .find_one(doc! {"_id":&candidate.id})
            .session(&mut *session)
            .await?;
        let intent = if let Some(document) = retained {
            let intent = read_intent(&document)?;
            intent
                .callback
                .require_alias(&candidate.callback)
                .map_err(|_| CallbackError::ReplayConflict)?;
            if intent.request != candidate.request
                || acceptance != CallbackAcceptance::ControlRefused(intent.receipt.clone())
            {
                return Err(CallbackError::ReplayConflict.into());
            }
            Some(intent)
        } else if prior.is_none() {
            if acceptance != CallbackAcceptance::ControlRefused(candidate.receipt.clone()) {
                return Err(CallbackError::ReplayConflict.into());
            }
            intents
                .insert_one(intent_document(candidate)?)
                .session(&mut *session)
                .await?;
            Some(candidate.clone())
        } else {
            // Old Message/refusal history wins. Do not backfill an old refusal
            // from a newer source observation, reference or acceptance time.
            None
        };
        if let Some(intent) = &intent {
            let alias = alias_document(&candidate.callback, intent)?;
            if let Some(existing) = aliases
                .find_one(doc! {"_id":&candidate.callback.callback_id})
                .session(&mut *session)
                .await?
            {
                require_alias(&existing, &candidate.callback, intent)?;
            } else {
                aliases.insert_one(alias).session(&mut *session).await?;
            }
        }
        write.inbox().require_current(write.inbox().bridge())?;
        Ok(RefusedControlAcceptance { acceptance, intent })
    }

    async fn recover_refused_control(
        &self,
        write: &ChannelControlWrite,
    ) -> Result<RefusedControlAcceptance, CallbackError> {
        let acceptance = self.reconcile_commit(write.inbox()).await?;
        let callback =
            ChannelControlCallback::capture(write).map_err(|_| CallbackError::Unavailable)?;
        let id = callback
            .intent_id()
            .map_err(|_| CallbackError::Unavailable)?;
        let intent = self
            .control_collection("bridge_refused_control_intents")
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| CallbackError::Unavailable)?
            .as_ref()
            .map(read_intent)
            .transpose()?;
        if let Some(intent) = &intent {
            let alias = self
                .control_collection("bridge_refused_control_aliases")
                .find_one(doc! {"_id":&callback.callback_id})
                .await
                .map_err(|_| CallbackError::Unavailable)?
                .ok_or(CallbackError::Unavailable)?;
            require_alias(&alias, &callback, intent)?;
            if acceptance != CallbackAcceptance::ControlRefused(intent.receipt.clone()) {
                return Err(CallbackError::ReplayConflict);
            }
        }
        Ok(RefusedControlAcceptance { acceptance, intent })
    }

    fn control_collection(&self, name: &str) -> Collection<Document> {
        majority_collection(
            &self.bridges.client.database(&self.inbox.namespace().db),
            name,
        )
    }
}

fn intent_document(intent: &RefusedChannelControlIntent) -> Result<Document, CallbackError> {
    intent.validate().map_err(|_| CallbackError::Unavailable)?;
    Ok(
        doc! {"_id":&intent.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "record_sha256":layer2_core::profiles::content_digest(intent).map_err(|_| CallbackError::Unavailable)?,
        "record":record_bson(intent).map_err(|_| CallbackError::Unavailable)?},
    )
}

fn read_intent(document: &Document) -> Result<RefusedChannelControlIntent, CallbackError> {
    let intent: RefusedChannelControlIntent =
        read_record(document).map_err(|_| CallbackError::Unavailable)?;
    let expected = intent_document(&intent)?;
    if document.get("_id") != expected.get("_id")
        || document.get("record_sha256") != expected.get("record_sha256")
    {
        return Err(CallbackError::Unavailable);
    }
    Ok(intent)
}

fn alias_document(
    callback: &ChannelControlCallback,
    intent: &RefusedChannelControlIntent,
) -> Result<Document, CallbackError> {
    intent
        .callback
        .require_alias(callback)
        .map_err(|_| CallbackError::ReplayConflict)?;
    let alias = Alias {
        schema_version: 1,
        callback: callback.clone(),
        intent_id: intent.id.clone(),
        intent_sha256: layer2_core::profiles::content_digest(intent)
            .map_err(|_| CallbackError::Unavailable)?,
    };
    Ok(
        doc! {"_id":&callback.callback_id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "record":record_bson(&alias).map_err(|_| CallbackError::Unavailable)?},
    )
}

fn require_alias(
    document: &Document,
    callback: &ChannelControlCallback,
    intent: &RefusedChannelControlIntent,
) -> Result<(), CallbackError> {
    let alias: Alias = read_record(document).map_err(|_| CallbackError::Unavailable)?;
    alias
        .callback
        .require_callback(callback)
        .map_err(|_| CallbackError::ReplayConflict)?;
    let expected = alias_document(&alias.callback, intent)?;
    if document.get("_id") != expected.get("_id")
        || document.get("record") != expected.get("record")
    {
        return Err(CallbackError::Unavailable);
    }
    Ok(())
}
