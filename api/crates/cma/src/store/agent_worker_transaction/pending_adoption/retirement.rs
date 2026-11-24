//! Close an expired publication by proving its preimage and fencing its commit.
use super::*;
use crate::store::transaction_terminal::GuardedTerminal as _;

pub(in crate::store) async fn retire_expired(
    store: &MongoSandboxStore,
    row: &Document,
    agent: &Agent,
    now: &str,
) -> Result<bool, SandboxStoreError> {
    let Some(witness) = Witness::read(row)
        .map_err(|_| witness::unavailable())?
        .filter(|witness| {
            witness.state == State::Admitted
                && witness::expired(witness, now)
                && !witness.lease_live()
        })
    else {
        return Ok(false);
    };
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| witness::unavailable())?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(|_| witness::unavailable())?;
    let result = retire(store, agent, &witness, &mut scope).await;
    if !matches!(result, Ok(true)) {
        scope
            .release()
            .abort_guarded()
            .await
            .map_err(|_| witness::unavailable())?;
        return result.map_err(|_| witness::unavailable());
    }
    match commit_transaction(scope.release()).await {
        CommitOutcome::Committed => {
            cohort(Stage::Blocked, Outcome::KnownAbort);
            Ok(true)
        }
        CommitOutcome::Retry(_) => Ok(false),
        CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => Err(witness::unavailable()),
    }
}

async fn retire(
    store: &MongoSandboxStore,
    agent: &Agent,
    witness: &Witness,
    scope: &mut TransactionScope<'_>,
) -> Result<bool, ApplyError> {
    let row = scope
        .find_one(&store.agents, doc! {"_id":agent.id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(witness::unresolved)?;
    let current = read_agent(&row).map_err(ApplyError::Domain)?;
    if Witness::read(&row)?.as_ref() != Some(witness)
        || crate::store::agent_log_segments::read_format(&row)
            .map_err(ApplyError::Domain)?
            .is_some()
        || !witness::matches_source(witness, &current)?
    {
        return Ok(false);
    }
    let operation = witness.operation();
    let database = store.client.database(&store.agents.namespace().db);
    let receipt = database.collection::<Document>(crate::store::agent_log_segments::PUBLICATIONS);
    let id = publication::identity(current.id.as_str(), &incarnation(&current), &operation.key);
    if scope
        .find_one(&receipt, doc! {"_id": id})
        .await
        .map_err(ApplyError::Driver)?
        .is_some()
    {
        // Completion updates the witness and format in the same transaction.
        // A receipt with an admitted legacy witness cannot authorize replacement.
        return Err(witness::unresolved());
    }
    let mut retired = witness.clone();
    retired.state = State::Retired;
    let lease = current
        .work
        .lease
        .as_ref()
        .ok_or_else(witness::unresolved)?;
    let revision = signed(current.revision, "adoption retirement revision")
        .map_err(|_| witness::unresolved())?;
    let sequence = signed(
        current.conversation.last_seq,
        "adoption retirement sequence",
    )
    .map_err(|_| witness::unresolved())?;
    let fence =
        signed(lease.fence, "adoption retirement fence").map_err(|_| witness::unresolved())?;
    let result = scope.update_one(&store.agents,
        doc! {"_id": current.id.as_str(), witness::FIELD: witness.document()?,
            "document_schema_version":DOCUMENT_SCHEMA_VERSION,"revision":revision,"record.revision":revision,
            "last_seq":sequence,"record.conversation.last_seq":sequence,"work_lease_fence":fence,
            "work_lease_holder": &lease.holder,"work_lease_expires_at":&lease.expires_at,
            "$expr":{"$not":[witness.lease_server_expression()]}},
        doc! {"$set":{witness::FIELD:retired.document()?}},
    ).await.map_err(ApplyError::Driver)?;
    Ok(result.matched_count == 1)
}
