//! Whole-Agent sequence-domain proof, independent of lifetime source length.
use super::capture::preview_budget::PreviewBudget;
use super::{corrupt, Head};
use crate::store::{
    agent_log_integrity::TransactionLogError, durability::majority_collection,
    migrations::agent_log_source_keys,
};
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{
    bson::{doc, Document},
    options::{Collation, Hint, ReadConcern},
    ClientSession, Database,
};
use std::time::Duration;

pub(super) async fn prove(
    database: &Database,
    head: &Head,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<(), TransactionLogError> {
    prove_budgeted(database, head, session, snapshot, None).await
}

pub(super) async fn prove_budgeted(
    database: &Database,
    head: &Head,
    session: &mut ClientSession,
    snapshot: bool,
    mut budget: Option<&mut PreviewBudget>,
) -> Result<(), TransactionLogError> {
    // Catalog qualification is outside the data snapshot, as for the opening
    // index. Every source query below joins the caller's original session.
    let pending = if let Some(budget) = budget.as_deref_mut() {
        budget
            .catalog(agent_log_source_keys::inspect(database))
            .await?
    } else {
        agent_log_source_keys::inspect(database).await
    }
    .map_err(|reason| SandboxStoreError::Unavailable(reason.into()))?;
    if !pending.is_empty() {
        return Err(SandboxStoreError::Unavailable(
            "Agent log source-key index migration is required".into(),
        )
        .into());
    }
    let source = majority_collection::<Document>(database, "agent_log");
    // Under simple binary collation, [id, id + NUL) contains exactly id,
    // including arbitrary UTF-8 and embedded NUL. A hint alone can scan every
    // foreign malformed key when Mongo cannot infer the partial predicate.
    let malformed = source
        .find_one(doc! {"agent_id":&head.agent_id})
        .hint(Hint::Name(agent_log_source_keys::INDEX.into()))
        .min(doc! {"agent_id":&head.agent_id})
        .max(doc! {"agent_id":format!("{}\0", head.agent_id)})
        .collation(Collation::builder().locale("simple").build())
        .projection(doc! {"_id":0,"found":{"$literal":true}})
        .max_time(Duration::from_secs(5));
    let malformed = if snapshot {
        malformed.read_concern(ReadConcern::snapshot())
    } else {
        malformed
    };
    let malformed = malformed
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if let Some(budget) = budget.as_deref_mut() {
        match &malformed {
            Some(row) => budget.charge_document(row)?,
            None => budget.charge(5)?,
        }
    }
    if malformed.is_some() {
        return Err(corrupt().into());
    }
    let outside = source
        .find_one(doc! {"agent_id":&head.agent_id,"$or":[
            {"seq":{"$lt":1_i64}},{"seq":{"$gt":head.last_seq}},
        ]})
        .hint(Hint::Name("agent_log_sequence".into()))
        .collation(Collation::builder().locale("simple").build())
        .projection(doc! {"_id":0,"found":{"$literal":true}})
        .max_time(Duration::from_secs(5));
    let outside = if snapshot {
        outside.read_concern(ReadConcern::snapshot())
    } else {
        outside
    };
    let outside = outside
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?;
    if let Some(budget) = budget {
        match &outside {
            Some(row) => budget.charge_document(row)?,
            None => budget.charge(5)?,
        }
    }
    if outside.is_some() {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    Ok(())
}
