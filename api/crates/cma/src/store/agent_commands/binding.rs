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
    if let Some(fence) = mutation
        .channel_execution_fence
        .as_ref()
        .filter(|fence| fence.superseding_command().is_some())
    {
        let invokes = store
            .channel_invokes
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or(CommandLedgerError::NotAdmissible)?;
        invokes
            .fence_channel_supersession(fence, current, mutation, session)
            .await
            .map_err(|_| CommandLedgerError::Conflict)?;
        return Ok(None);
    }
    let Some(claim) = &mutation.command_claim else {
        return Ok(None);
    };
    let now = layer2_core::now_rfc3339();
    let (mut command, lane) = worker_apply::load_claim(store, claim, &now, session).await?;
    let revision = command.revision;
    if let Some(fence) = &mutation.channel_control_execution_fence {
        let invokes = store
            .channel_invokes
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or(CommandLedgerError::NotAdmissible)?;
        invokes
            .fence_channel_control_execution(fence, &command, session)
            .await
            .map_err(|error| {
                use super::super::bridge_invokes::channel_control_execution::MutationError;
                match error {
                    MutationError::Driver(driver) => ApplyError::Driver(driver),
                    MutationError::Core(_) => ApplyError::Domain(CommandLedgerError::Conflict),
                }
            })?;
    }
    if let Some(fence) = &mutation.channel_execution_fence {
        let invokes = store
            .channel_invokes
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or(CommandLedgerError::NotAdmissible)?;
        invokes
            .fence_channel_execution(fence, &command, session)
            .await
            .map_err(|_| CommandLedgerError::Conflict)?;
        let changed = store.agent_commands.update_one(
            doc! {"_id": command.receipt.envelope.command_id.as_str(), "revision": command.revision as i64},
            doc! {"$set": {"channel_dispatch_authority": {
                "grant_id": fence.proof().grant().id.as_str(),
                "grant_revision": fence.proof().grant().revision as i64,
                "bridge_revision": fence.proof().bridge().revision as i64,
                "claim_fence": claim.fence as i64,
                "at": &mutation.appended_at,
            }}},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(CommandLedgerError::Conflict.into());
        }
    }
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
