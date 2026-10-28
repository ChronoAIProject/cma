//! Preflight reads, current authority check, and a single bounded platform write.
use super::*;

impl ToolsState {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn perform_thread_mutation(
        &self,
        caller: AgentCaller,
        channel: &ResolvedChannel,
        authority: &Authority,
        target: &Identity,
        operation: &Operation,
        client: &DiscordThreads,
        budget: &Budget,
    ) -> Result<Option<Thread>, ThreadOutcome> {
        tokio::time::timeout_at(
            budget.preflight_end(),
            self.thread_preflight(channel, authority, target, operation, client),
        )
        .await
        .map_err(|_| outcome(RestError::Unavailable))??;
        // All bot verification and metadata reads precede this fresh owner/source/consumer check.
        let current = tokio::time::timeout_at(
            budget.preflight_end(),
            self.recheck_thread_authority(caller, authority),
        )
        .await
        .map_err(|_| outcome(RestError::Unavailable))?
        .map_err(|_| revoked())?;
        let credential = token(&current, &authority.bot).ok_or_else(revoked)?;
        budget.dispatch()?;
        match operation {
            Operation::Manage { action } => tokio::time::timeout_at(
                budget.write_end(),
                client.api.manage_thread(credential, target, *action),
            )
            .await
            .map_err(|_| outcome(RestError::Unknown))?
            .map_err(outcome),
            Operation::Create { request } => match tokio::time::timeout_at(
                budget.write_end(),
                client
                    .api
                    .create_thread(credential, target, &authority.bot, request),
            )
            .await
            .unwrap_or(Err(RestError::Unknown))
            {
                Ok(thread) => Ok(Some(thread)),
                Err(RestError::Unknown) => {
                    // Reconciliation is authenticated and destination-exact. No absence/error reopens the fence.
                    let fresh = self
                        .recheck_thread_authority(caller, authority)
                        .await
                        .map_err(|_| outcome(RestError::Unknown))?;
                    let credential =
                        token(&fresh, &authority.bot).ok_or_else(|| outcome(RestError::Unknown))?;
                    tokio::time::timeout_at(
                        budget.write_end(),
                        client.api.reconcile_created_thread(
                            credential,
                            target,
                            &authority.bot,
                            request,
                        ),
                    )
                    .await
                    .map_err(|_| outcome(RestError::Unknown))?
                    .map(Some)
                    .map_err(|_| outcome(RestError::Unknown))
                }
                Err(error) => Err(outcome(error)),
            },
        }
    }
    async fn thread_preflight(
        &self,
        channel: &ResolvedChannel,
        authority: &Authority,
        target: &Identity,
        operation: &Operation,
        client: &DiscordThreads,
    ) -> Result<(), ThreadOutcome> {
        let credential = token(channel, &authority.bot).ok_or_else(revoked)?;
        client
            .api
            .verify_bot(credential, &authority.bot)
            .await
            .map_err(outcome)?;
        client
            .api
            .check_thread_origin(credential, &authority.origin)
            .await
            .map_err(outcome)?;
        match operation {
            Operation::Create { .. } => {
                let message = client
                    .api
                    .message(credential, &authority.origin.id, &authority.message)
                    .await
                    .map_err(outcome)?;
                if !thread_authority::human_message(&message, &authority.activity.actor.id) {
                    return Err(revoked());
                }
                match client.api.thread_channel(credential, &target.id).await {
                    Ok(_) => {
                        return Err(ThreadOutcome::Rejected {
                            reason: "thread_already_exists".into(),
                        })
                    }
                    Err(RestError::NotFound) => {}
                    Err(error) => return Err(outcome(error)),
                }
            }
            Operation::Manage { action } => {
                let value = client
                    .api
                    .thread_channel(credential, &target.id)
                    .await
                    .map_err(outcome)?;
                let thread = Thread::parse(&value, target).map_err(outcome)?;
                if thread.archived && matches!(action, ThreadAction::Join | ThreadAction::Leave) {
                    return Err(ThreadOutcome::Unsupported {
                        reason: "membership_requires_active_thread".into(),
                    });
                }
            }
        }
        Ok(())
    }
}
