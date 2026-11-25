//! Invalidate a certified active tail atomically with deletion of its source.
use super::Error;
use crate::store::durability::majority_collection;
use crate::store::{
    agent_log_segments::active_proof::COLLECTION,
    sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
    transaction_terminal::GuardedTerminal as _,
};
use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};
use std::collections::BTreeSet;

pub(in crate::store) async fn erase_source_log(
    db: &Database,
    rows: &[Document],
    ids: &[Bson],
    filter: Document,
) -> Result<u64, Error> {
    let source = majority_collection::<Document>(db, "agent_log");
    if super::optional::absent(db, COLLECTION, Some(false)).await? {
        return Ok(source.delete_many(filter).await?.deleted_count);
    }
    let agents: BTreeSet<_> = rows
        .iter()
        .filter(|row| row.get("_id").is_some_and(|id| ids.contains(id)))
        .map(|row| {
            row.get_str("agent_id")
                .map(str::to_owned)
                .map_err(|_| Error::InvalidRecord("agent_log"))
        })
        .collect::<Result<_, _>>()?;
    let mut session = db.client().start_session().await?;
    start_transaction(&mut session).await?;
    let result = async {
        let proofs = majority_collection::<Document>(db, COLLECTION);
        // An upsert, including when no prior proof exists, shares the same
        // unique identity with first preparation and every successor proof.
        for agent in agents {
            let fence = doc! {"_id": &agent, "format_version":0_i64,"invalidated_for_erasure":true};
            proofs
                .replace_one(doc! {"_id":&agent}, fence)
                .upsert(true)
                .session(&mut session)
                .await?;
        }
        let removed = source
            .delete_many(filter)
            .session(&mut session)
            .await?
            .deleted_count;
        Ok::<_, mongodb::error::Error>(removed)
    }
    .await;
    let removed = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = session.abort_guarded().await;
            return Err(error.into());
        }
    };
    if !matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ) {
        return Err(Error::Store(
            "Agent log erasure transaction outcome unresolved".into(),
        ));
    }
    Ok(removed)
}
