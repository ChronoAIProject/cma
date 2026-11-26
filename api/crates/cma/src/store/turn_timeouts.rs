//! Policy terminal publication shares the conversation writer transaction.
use layer2_core::sandbox::{store::SandboxStoreError, turn_timeout::TurnTimeoutSettlement};
use mongodb::bson::{doc, Document};

use super::{
    agent_commands::records,
    agent_conversation_store::ApplyError,
    agent_log_segments::{conversation::checkpoint, transaction_scope::TransactionScope},
    sandbox_mongo::{commit_transaction, signed, CommitOutcome, MongoSandboxStore},
    transaction_terminal::GuardedTerminal as _,
};

const RECEIPT: &str = "turn_policy_publication";
pub(crate) mod discovery;
pub(in crate::store) mod floor;

pub(crate) async fn settle(
    store: &MongoSandboxStore,
    proof: &TurnTimeoutSettlement,
) -> Result<(), SandboxStoreError> {
    if confirmed(store, proof).await? {
        return Ok(());
    }
    let _writer = store.agent_writes.lock(&proof.snapshot().agent.id).await;
    let _platform = store.conversation_platform_guard(proof.mutation()).await;
    let mut session = store.client.start_session().await.map_err(unavailable)?;
    let mut scope = TransactionScope::start(&mut session)
        .await
        .map_err(unavailable)?;
    let applied = Box::pin(apply(store, proof, &mut scope)).await;
    let session = scope.release();
    if let Err(error) = applied {
        if let Err(abort) = session.abort_guarded().await {
            tracing::warn!(%abort, "turn policy transaction abort unconfirmed");
        }
        return Err(match error {
            ApplyError::Domain(error) => error,
            ApplyError::Driver(error) => unavailable(error),
        });
    }
    match commit_transaction(session).await {
        CommitOutcome::Committed => {
            crate::telemetry::work::published(&proof.checkpoint().outcome.appended, "committed");
            Ok(())
        }
        CommitOutcome::Unknown(_) => {
            // A majority receipt can prove the action committed. Absence never
            // proves abortion; leave the same action for the next reconciliation.
            if confirmed(store, proof).await? {
                Ok(())
            } else {
                Err(unavailable("turn policy commit outcome unconfirmed"))
            }
        }
        CommitOutcome::Retry(_) => Err(SandboxStoreError::Conflict),
        CommitOutcome::Failed(error) => Err(unavailable(error)),
    }
}

async fn apply(
    store: &MongoSandboxStore,
    proof: &TurnTimeoutSettlement,
    scope: &mut TransactionScope<'_>,
) -> Result<(), ApplyError> {
    let conflict = || ApplyError::Domain(SandboxStoreError::Conflict);
    let current = &proof.snapshot().agent;
    let id = &proof.command().receipt.envelope.command_id;
    let command = scope
        .find_one(&store.agent_commands, doc! {"_id":id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(conflict)?;
    if records::read(&command).map_err(|_| conflict())? != *proof.command()
        || command.contains_key(RECEIPT)
        || command.contains_key("channel_history_seal")
    {
        return Err(conflict());
    }
    let parent_row = scope
        .find_one(&store.sandboxes, doc! {"_id":current.sandbox_id.as_str()})
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(conflict)?;
    let parent = super::sandboxes::read_sandbox(&parent_row).map_err(ApplyError::Domain)?;
    if parent.owner != current.owner
        || matches!(
            parent.status,
            layer2_core::sandbox::SandboxStatus::Retiring
                | layer2_core::sandbox::SandboxStatus::Retired
        )
    {
        return Err(conflict());
    }
    let locked = scope.update_one(&store.sandboxes, doc! {
        "_id":parent.id.as_str(), "revision":signed(parent.revision,"turn policy parent revision").map_err(|_| conflict())?
    }, doc! {"$inc":{"turn_policy_parent_fence":1_i64}}).await.map_err(ApplyError::Driver)?;
    if locked.matched_count != 1 {
        return Err(conflict());
    }
    if scope
        .find_projected(
            &store.agent_commands,
            doc! {
                "agent_id":current.id.as_str(), "_id":{"$ne":id.as_str()},
                "status":{"$in":["queued","in_progress","waiting_action"]},
                "record.receipt.envelope.command.type":{"$ne":"message"}
            },
            doc! {"_id":1},
        )
        .await
        .map_err(ApplyError::Driver)?
        .is_some()
    {
        return Err(conflict());
    }
    let settled = Box::pin(checkpoint::apply_turn_policy(store, proof, scope)).await?;
    let receipt = receipt(proof).map_err(ApplyError::Domain)?;
    let updated = scope.update_one(&store.agent_commands, doc! {
        "_id":id.as_str(), "revision":signed(settled.revision,"turn policy command revision").map_err(|_| conflict())?, RECEIPT:{"$exists":false}
    }, doc! {"$set":{RECEIPT:receipt}}).await.map_err(ApplyError::Driver)?;
    if updated.matched_count != 1 {
        return Err(conflict());
    }
    Ok(())
}

fn receipt(proof: &TurnTimeoutSettlement) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "version":1_i32, "action_id":&proof.checkpoint().update_id,
        "fingerprint":&proof.checkpoint().update_fingerprint,
        "agent_id":proof.snapshot().agent.id.as_str(),
        "turn_sequence":signed(proof.command().work.dispatch.as_ref().ok_or(SandboxStoreError::Conflict)?.turn_sequence,"turn policy sequence").map_err(SandboxStoreError::Corrupt)?,
    })
}

async fn confirmed(
    store: &MongoSandboxStore,
    proof: &TurnTimeoutSettlement,
) -> Result<bool, SandboxStoreError> {
    let Some(row) = store
        .agent_commands
        .find_one(doc! {"_id":proof.command().receipt.envelope.command_id.as_str()})
        .max_time(std::time::Duration::from_secs(1))
        .await
        .map_err(unavailable)?
    else {
        return Ok(false);
    };
    let Some(actual) = row.get(RECEIPT) else {
        return Ok(false);
    };
    if actual.as_document() != Some(&receipt(proof)?)
        || records::read(&row)
            .map_err(|_| SandboxStoreError::Conflict)?
            .status
            != layer2_core::sandbox::agent_io::AgentResponseStatus::Failed
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(true)
}

fn unavailable(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}
