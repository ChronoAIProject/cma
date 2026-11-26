//! Admit catalogue Bot pins atomically with its durable creation receipt.
use super::*;
use crate::store::sandbox_mongo::{
    commit_transaction, is_duplicate_key, start_transaction, CommitOutcome,
};
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(super) async fn insert(store: &MongoBotStore, bot: &Bot) -> Result<(), BotError> {
    let row = BotDocument::new(bot)?;
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| BotError::Unavailable)?;
    start_transaction(&mut session)
        .await
        .map_err(|_| BotError::Unavailable)?;
    match apply(store, bot, &row, &mut session).await {
        Ok(true) => {}
        Ok(false) => {
            let _ = session.abort_guarded().await;
            return Err(BotError::Conflict);
        }
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(if is_duplicate_key(&error) {
                BotError::Conflict
            } else {
                BotError::Unavailable
            });
        }
    }
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(()),
        CommitOutcome::Unknown(_) => {
            let current = store.creation(&bot.owner, &bot.creation_key_hash).await?;
            if current.is_some_and(|current| {
                current.bot_id == bot.bot_id
                    && current.request_fingerprint == bot.request_fingerprint
            }) {
                Ok(())
            } else {
                Err(BotError::Unavailable)
            }
        }
        CommitOutcome::Retry(_) | CommitOutcome::Failed(_) => Err(BotError::Unavailable),
    }
}

async fn apply(
    store: &MongoBotStore,
    bot: &Bot,
    row: &BotDocument,
    session: &mut mongodb::ClientSession,
) -> mongodb::error::Result<bool> {
    // Duplicate creation is resolved by the durable receipt before new pins.
    if store
        .records
        .find_one(doc! {"$or":[{"_id":&bot.bot_id},
            {"owner":bot.owner.as_str(),"creation_key_hash":&bot.creation_key_hash}]
        })
        .session(&mut *session)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    let db = store.client.database(&store.records.namespace().db);
    if !crate::store::profile_pin_admission::bot_profiles(&db, &bot.profiles, session).await? {
        return Ok(false);
    }
    store
        .records
        .insert_one(row.clone())
        .session(session)
        .await?;
    Ok(true)
}
