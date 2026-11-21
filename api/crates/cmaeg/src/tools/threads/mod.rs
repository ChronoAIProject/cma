//! Bounded original-event thread lifecycle, shared by REST and MCP.
mod budget;
mod create;
mod errors;
mod execute;
mod ledger;
mod topics;
use super::ToolsState;
use crate::discord::mutation::hash;
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    discord::{
        rest::{threads::Thread, token},
        thread_authority::{self, Authority, Identity},
        DiscordRest, RestError,
    },
};
use budget::Budget;
use cmaeg_core::{
    channel::Source,
    problem::{Problem, ProblemCode},
    seal::{EventRef, EventRefPayload, ReplyAddress, ThreadRef, ThreadRefPayload},
    thread_lifecycle::{
        CreateThreadRequest, ManageThreadRequest, ThreadAction, ThreadCapabilities, ThreadList,
        ThreadListScope, ThreadListView, ThreadOutcome, ThreadReceipt, ThreadSummary, ThreadTarget,
    },
};
use errors::*;
use serde::Serialize;
use std::sync::Arc;
use thread_authority::{denied, unavailable};
use tokio::sync::Semaphore;
pub use topics::TelegramTopics;

pub struct DiscordThreads {
    api: DiscordRest,
    slots: Semaphore,
    budget: std::time::Duration,
}
impl DiscordThreads {
    pub fn new(api: DiscordRest) -> Self {
        Self {
            api,
            slots: Semaphore::new(8),
            budget: budget::TOTAL,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Operation {
    Create {
        request: CreateThreadRequest,
    },
    Manage {
        action: ThreadAction,
        // Preserve every pre-rename operation's serialized bytes and durable claim hash.
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl ToolsState {
    fn thread_client(&self) -> Result<&DiscordThreads, Problem> {
        self.discord_threads
            .as_deref()
            .ok_or_else(|| Problem::new(ProblemCode::Forbidden, 403, "thread_lifecycle_disabled"))
    }
    async fn thread_authority(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
    ) -> Result<(Arc<ResolvedChannel>, Authority), Problem> {
        self.thread_client()?;
        let channel = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        let authority = self
            .thread_authority_in_channel(caller, event, &channel)
            .await?;
        Ok((channel, authority))
    }
    async fn thread_authority_in_channel(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        channel: &ResolvedChannel,
    ) -> Result<Authority, Problem> {
        super::state::owner(caller, channel, event.consumer.as_ref(), "threadLifecycle")?;
        if !self.resolver.record_client().thread_fences_ready().await {
            return Err(unavailable());
        }
        thread_authority::load(
            self.resolver.record_client(),
            self.keys.clone(),
            channel,
            event,
        )
        .await
    }
    pub(super) async fn thread_capabilities(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        channel: &ResolvedChannel,
    ) -> Option<ThreadCapabilities> {
        let client = self.discord_threads.as_deref()?;
        let ReplyAddress::Discord { bot_id, .. } = &event.reply else {
            return None;
        };
        if event.lineage.is_some() || event.expires_at <= now()
            || !channel.definition.sources.iter().any(|source| {
                matches!(source, Source::Discord { bot_id: configured, thread_lifecycle: true, .. } if configured == bot_id)
            })
        {
            return None;
        }
        token(channel, bot_id)?;
        let _permit = client.slots.try_acquire().ok()?;
        // Reuse the authorized context snapshot. Explicit operations refresh it before writing.
        let authority = tokio::time::timeout(
            budget::DISCOVERY,
            self.thread_authority_in_channel(caller, event, channel),
        )
        .await
        .ok()?
        .ok()?;
        Some(ThreadCapabilities {
            create_from_original_message: !authority.origin.thread(),
            list: scope(&authority),
            manage_original_thread: authority.origin.thread(),
            actions: vec![
                ThreadAction::Archive,
                ThreadAction::Reopen,
                ThreadAction::Join,
                ThreadAction::Leave,
                ThreadAction::Rename,
            ],
            expires_at: event.expires_at,
        })
    }
    pub async fn list_event_threads(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        view: ThreadListView,
    ) -> Result<ThreadList, Problem> {
        let budget = Budget::new(self.thread_client()?.budget);
        tokio::time::timeout_at(
            budget.end,
            self.list_event_threads_inner(caller, reference, view),
        )
        .await
        .map_err(|_| unavailable())?
    }
    async fn list_event_threads_inner(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        view: ThreadListView,
    ) -> Result<ThreadList, Problem> {
        let client = self.thread_client()?;
        let _permit = client.slots.try_acquire().map_err(|_| busy())?;
        let event = reference
            .open(&self.keys, now())
            .map_err(|_| invalid_reference())?
            .payload;
        let (channel, authority) = self.thread_authority(caller, &event).await?;
        let credential = token(&channel, &authority.bot).ok_or_else(denied)?;
        client
            .api
            .verify_bot(credential, &authority.bot)
            .await
            .map_err(read_problem)?;
        let (threads, truncated) = client
            .api
            .list_threads(credential, &authority.origin, view)
            .await
            .map_err(read_problem)?;
        self.recheck_thread_authority(caller, &authority).await?;
        Ok(ThreadList {
            threads: threads
                .iter()
                .map(|thread| self.thread_summary(&authority, thread))
                .collect::<Result<_, _>>()?,
            truncated,
            scope: scope(&authority),
        })
    }
    pub async fn manage_thread(
        &self,
        caller: AgentCaller,
        request: ManageThreadRequest,
        key: &str,
    ) -> Result<ThreadReceipt, Problem> {
        super::validate_reply_key(Some(key))?;
        request
            .validate()
            .map_err(|e| Problem::new(ProblemCode::InvalidRequest, 400, e.to_string()))?;
        if let ThreadTarget::Event { event_ref } = &request.target {
            let event = event_ref
                .open(&self.keys, now())
                .map_err(|_| invalid_reference())?
                .payload;
            if matches!(event.reply, ReplyAddress::Telegram { .. }) {
                return self
                    .manage_telegram_topic(
                        caller,
                        event,
                        request.action,
                        request.name.as_deref(),
                        key,
                    )
                    .await;
            }
        }
        if let ThreadTarget::Thread { thread_ref } = &request.target {
            if let Ok(reference) = thread_ref.open_telegram(&self.keys, now()) {
                return self
                    .manage_created_telegram_topic(
                        caller,
                        reference.payload,
                        request.action,
                        request.name.as_deref(),
                        key,
                    )
                    .await;
            }
        }
        // Reject Telegram-only close before Discord metadata reads or mutations.
        if request.action == ThreadAction::Close {
            return Ok(receipt(ThreadOutcome::Unsupported {
                reason: "close_requires_telegram_topic_event".into(),
            }));
        }
        let budget = Budget::new(self.thread_client()?.budget);
        tokio::time::timeout_at(
            budget.end,
            self.manage_thread_inner(caller, request, key, &budget),
        )
        .await
        .unwrap_or_else(|_| Ok(receipt(budget.expired())))
    }
    async fn manage_thread_inner(
        &self,
        caller: AgentCaller,
        request: ManageThreadRequest,
        key: &str,
        budget: &Budget,
    ) -> Result<ThreadReceipt, Problem> {
        super::validate_reply_key(Some(key))?;
        let (event, sealed) = match request.target {
            ThreadTarget::Event { event_ref } => (
                event_ref
                    .open(&self.keys, now())
                    .map_err(|_| invalid_reference())?
                    .payload,
                None,
            ),
            ThreadTarget::Thread { thread_ref } => {
                let payload = thread_ref
                    .open(&self.keys, now())
                    .map_err(|_| invalid_reference())?
                    .payload;
                (payload.event.clone(), Some(payload))
            }
        };
        let (channel, authority) = self.thread_authority(caller, &event).await?;
        let target = match sealed {
            Some(reference) => authority.target(&reference)?,
            None if authority.origin.thread() => authority.origin.clone(),
            None => {
                return Ok(receipt(ThreadOutcome::Unsupported {
                    reason: "manage_requires_original_thread_or_scoped_thread_ref".into(),
                }))
            }
        };
        self.mutate_thread(
            caller,
            channel,
            authority,
            target,
            Operation::Manage {
                action: request.action,
                name: request.name,
            },
            key,
            budget,
        )
        .await
    }
    async fn recheck_thread_authority(
        &self,
        caller: AgentCaller,
        original: &Authority,
    ) -> Result<Arc<ResolvedChannel>, Problem> {
        let (channel, current) = self.thread_authority(caller, &original.event).await?;
        if current.custody != original.custody || current.origin != original.origin {
            return Err(denied());
        }
        Ok(channel)
    }
    fn thread_summary(
        &self,
        authority: &Authority,
        thread: &Thread,
    ) -> Result<ThreadSummary, Problem> {
        let payload = ThreadRefPayload {
            v: 1,
            event: authority.event.clone(),
            custody: authority.custody.clone(),
            guild_id: thread.identity.guild.clone(),
            parent_id: thread.identity.parent.clone().ok_or_else(denied)?,
            thread_id: thread.identity.id.clone(),
            thread_type: thread.identity.kind,
            issued_at: now(),
            expires_at: authority.event.expires_at,
        };
        authority.target(&payload)?;
        Ok(ThreadSummary {
            thread_ref: ThreadRef::seal(&payload, &self.keys).map_err(|_| unavailable())?,
            thread_id: thread.identity.id.clone(),
            parent_id: payload.parent_id,
            name: thread.name.clone(),
            archived: thread.archived,
            locked: thread.locked,
            expires_at: payload.expires_at,
        })
    }
    #[allow(clippy::too_many_arguments)]
    async fn mutate_thread(
        &self,
        caller: AgentCaller,
        channel: Arc<ResolvedChannel>,
        authority: Authority,
        target: Identity,
        operation: Operation,
        key: &str,
        budget: &Budget,
    ) -> Result<ThreadReceipt, Problem> {
        let client = self.thread_client()?;
        let _permit = client.slots.try_acquire().map_err(|_| busy())?;
        let store = ledger::Store::new(self.resolver.record_client(), self.keys.clone());
        let claim_scope = hash(&serde_json::json!({
            "event": authority.event, "custody": authority.custody, "target": target,
        }));
        budget.start_ledger();
        let claim = match store
            .claim(
                claim_scope,
                key,
                hash(&operation),
                &channel.definition.owner.subject,
                authority.event.expires_at,
            )
            .await?
        {
            ledger::Admission::Replay(saved) => return Ok(saved),
            ledger::Admission::Claimed(claim) => claim,
        };
        let Some(fence) = store
            .acquire(
                &authority.bot,
                &target.id,
                &channel.definition.owner.subject,
            )
            .await?
        else {
            let result = receipt(ThreadOutcome::Unknown {
                reason: "thread_mutation_in_progress_or_uncertain".into(),
            });
            store.finish(claim, &result).await?;
            return Ok(result);
        };
        let result = self
            .perform_thread_mutation(
                caller, &channel, &authority, &target, &operation, client, budget,
            )
            .await;
        let result = match result {
            Ok(thread) => {
                // A successful write may still lose its authority while in flight. Report the
                // confirmation but grant no fresh reference until current custody is verified.
                let summary = if self
                    .recheck_thread_authority(caller, &authority)
                    .await
                    .is_ok()
                {
                    thread
                        .as_ref()
                        .and_then(|thread| self.thread_summary(&authority, thread).ok())
                } else {
                    None
                };
                ThreadReceipt {
                    outcome: ThreadOutcome::Confirmed,
                    thread: summary,
                }
            }
            Err(outcome) => receipt(outcome),
        };
        if store.finish(claim, &result).await.is_err() {
            if !budget.has_dispatched() {
                // A proven no-write failure can safely release the destination even if its
                // individual claim receipt could not be saved. A failed release stays fenced.
                let _ = store.release(fence).await;
            }
            return Ok(receipt(ThreadOutcome::Unknown {
                reason: "thread_receipt_unconfirmed".into(),
            }));
        }
        if !matches!(result.outcome, ThreadOutcome::Unknown { .. })
            && store.release(fence).await.is_err()
        {
            tracing::warn!("thread mutation fence remains held after a completed receipt");
            if !budget.has_dispatched() {
                return Ok(receipt(ThreadOutcome::Unknown {
                    reason: "thread_cleanup_unconfirmed".into(),
                }));
            }
        }
        Ok(result)
    }
}
fn scope(authority: &Authority) -> ThreadListScope {
    if authority.origin.thread() {
        ThreadListScope::OriginalThread
    } else {
        ThreadListScope::PublicChildrenOfOriginalParent
    }
}
fn receipt(outcome: ThreadOutcome) -> ThreadReceipt {
    ThreadReceipt {
        outcome,
        thread: None,
    }
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod rename_tests;
#[cfg(test)]
mod tests;
