use layer2_core::sandbox::agent_io::{bind_command_dispatch, CommandDispatch, CommandLedgerError};
use layer2_core::sandbox::store::{AgentConversationMutation, SandboxStoreError};
use layer2_core::sandbox::Agent;
use mongodb::bson::doc;
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use super::super::sandbox_mongo::MongoSandboxStore;
use super::{records, worker_apply, ApplyError};

pub(in crate::store) async fn bind(
    store: &MongoSandboxStore,
    current: &Agent,
    latest_turn: Option<u64>,
    mutation: &AgentConversationMutation,
    session: &mut ClientSession,
) -> Result<Option<CommandDispatch>, ApplyError> {
    let Some(claim) = &mutation.command_claim else {
        return Ok(None);
    };
    let now = layer2_core::now_rfc3339();
    let (mut command, lane) = worker_apply::load_claim(store, claim, &now, session).await?;
    super::super::media_uploads::require_continuation(store, &command.receipt.envelope, session)
        .await?;
    let revision = command.revision;
    if let Some(fence) = &mutation.trigger_execution_fence {
        let proof =
            super::super::trigger_first_messages::proof_in_session(store, &command, session)
                .await
                .map_err(|_| CommandLedgerError::Conflict)?;
        fence
            .require_proof(&proof)
            .map_err(|_| CommandLedgerError::Conflict)?;
    }
    let dispatch =
        bind_command_dispatch(&mut command, &lane, current, latest_turn, mutation, &now)?;
    if let Some(fence) = &mutation.execution_fence {
        let evidence = fence
            .authorization_audit()
            .map_err(|_| CommandLedgerError::Conflict)?;
        let audit = layer2_core::sandbox::agent_io::CommandAuthorizationAudit::new(
            &command,
            layer2_core::sandbox::agent_io::CommandAuthorizationEffect::Dispatch {
                dispatch: dispatch.clone(),
            },
            vec![evidence],
        )?;
        super::audit::append(store, &audit, session).await?;
    }
    worker_apply::write(store, &command, revision, &lane, session).await?;
    Ok(Some(dispatch))
}

/// The command identity is part of ambiguous-commit proof. Two identical
/// controls may otherwise have indistinguishable Agent/log receipts.
pub(in crate::store) async fn matches_snapshot(
    store: &MongoSandboxStore,
    dispatch: &CommandDispatch,
    session: &mut ClientSession,
) -> Result<bool, SandboxStoreError> {
    let command = store
        .agent_commands
        .find_one(doc! { "_id": dispatch.claim.command_id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(command) = command else {
        return Ok(false);
    };
    let command = records::read(&command).map_err(domain_error)?;
    Ok(command.work.dispatch.as_ref() == Some(dispatch))
}

pub(in crate::store) fn domain_error(error: CommandLedgerError) -> SandboxStoreError {
    match error {
        CommandLedgerError::Corrupt => SandboxStoreError::Corrupt(error.to_string()),
        CommandLedgerError::Unavailable => SandboxStoreError::Unavailable(error.to_string()),
        _ => SandboxStoreError::Conflict,
    }
}
