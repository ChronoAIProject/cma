//! Event-only Telegram topic management, sharing the neutral REST/MCP entry point.
mod authority;
use super::*;
use crate::telegram::{topics::Mutation, Error as TelegramError, TelegramClient};
use cmaeg_core::thread_lifecycle::{TopicAction, TopicLifecycleCapabilities};

pub struct TelegramTopics {
    api: Arc<TelegramClient>,
    slots: Semaphore,
    budget: std::time::Duration,
}
impl TelegramTopics {
    pub fn new(api: Arc<TelegramClient>) -> Self {
        Self {
            api,
            slots: Semaphore::new(8),
            budget: budget::TOTAL,
        }
    }
}
impl ToolsState {
    fn topic_client(&self) -> Result<&TelegramTopics, Problem> {
        self.telegram_topics
            .as_deref()
            .ok_or_else(|| Problem::new(ProblemCode::Forbidden, 403, "topic_lifecycle_disabled"))
    }
    async fn topic_authority(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
    ) -> Result<(Arc<ResolvedChannel>, authority::Authority), Problem> {
        self.topic_client()?;
        let current = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|e| e.problem())?;
        super::super::state::owner(caller, &current, event.consumer.as_ref(), "topicLifecycle")?;
        if !self.resolver.record_client().thread_fences_ready().await {
            return Err(unavailable());
        }
        authority::load(self, caller, event, &current).await
    }
    pub(in crate::tools) async fn topic_capabilities(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        channel: &ResolvedChannel,
    ) -> Option<TopicLifecycleCapabilities> {
        let client = self.telegram_topics.as_deref()?;
        let ReplyAddress::Telegram {
            bot_id,
            thread_id: Some(topic),
            chat_id,
            ..
        } = &event.reply
        else {
            return None;
        };
        if !crate::telegram::topics::valid_topic(topic) || !crate::telegram::topics::valid_chat(chat_id)
            || !channel.definition.sources.iter().any(|s| matches!(s, Source::Telegram { bot_id: configured, topic_lifecycle: true, .. } if configured == bot_id))
        { return None; }
        let _permit = client.slots.try_acquire().ok()?;
        tokio::time::timeout(budget::DISCOVERY, self.topic_authority(caller, event))
            .await
            .ok()?
            .ok()?;
        Some(TopicLifecycleCapabilities {
            manage_original_topic: true,
            actions: vec![TopicAction::Close, TopicAction::Reopen],
            expires_at: event.expires_at,
        })
    }
    pub(super) async fn manage_telegram_topic(
        &self,
        caller: AgentCaller,
        event: EventRefPayload,
        action: ThreadAction,
        name: Option<&str>,
        key: &str,
    ) -> Result<ThreadReceipt, Problem> {
        super::super::validate_reply_key(Some(key))?;
        let action = match action {
            ThreadAction::Close => Mutation::Close,
            ThreadAction::Reopen => Mutation::Reopen,
            ThreadAction::Rename => Mutation::Rename {
                name: name.ok_or_else(|| {
                    Problem::new(ProblemCode::InvalidRequest, 400, "rename_name_required")
                })?,
            },
            _ => {
                return Ok(receipt(ThreadOutcome::Unsupported {
                    reason: "telegram_topic_action_unsupported".into(),
                }))
            }
        };
        let budget = Budget::new(self.topic_client()?.budget);
        tokio::time::timeout_at(
            budget.end,
            self.mutate_topic(caller, event, action, key, &budget),
        )
        .await
        .unwrap_or_else(|_| Ok(receipt(budget.expired())))
    }
    async fn recheck_topic_authority(
        &self,
        caller: AgentCaller,
        original: &authority::Authority,
    ) -> Result<Arc<ResolvedChannel>, Problem> {
        let (channel, current) = self.topic_authority(caller, &original.event).await?;
        if current.custody != original.custody {
            return Err(authority::denied());
        }
        Ok(channel)
    }
    async fn mutate_topic(
        &self,
        caller: AgentCaller,
        event: EventRefPayload,
        action: Mutation<'_>,
        key: &str,
        budget: &Budget,
    ) -> Result<ThreadReceipt, Problem> {
        let client = self.topic_client()?;
        let _permit = client.slots.try_acquire().map_err(|_| busy())?;
        let (channel, authority) = self.topic_authority(caller, &event).await?;
        let store = ledger::Store::telegram(self.resolver.record_client(), self.keys.clone());
        let scope = hash(
            &serde_json::json!({"channel":event.channel_id,"record":event.record_id,"event":event.event_id,"consumer":event.consumer,"platform":"telegram","bot":authority.bot,"chat":authority.chat,"topic":authority.topic}),
        );
        budget.start_ledger();
        let claim = match store
            .claim(
                scope,
                key,
                hash(&action),
                &channel.definition.owner.subject,
                event.expires_at,
            )
            .await?
        {
            ledger::Admission::Replay(saved) => return Ok(saved),
            ledger::Admission::Claimed(claim) => claim,
        };
        let Some(fence) = store
            .acquire_topic(
                &authority.bot,
                &authority.chat,
                &authority.topic,
                &channel.definition.owner.subject,
            )
            .await?
        else {
            let result = receipt(ThreadOutcome::Unknown {
                reason: "topic_mutation_in_progress_or_uncertain".into(),
            });
            store.finish(claim, &result).await?;
            return Ok(result);
        };
        let result = self
            .perform_topic_mutation(caller, &channel, &authority, action, client, budget)
            .await;
        let result = receipt(match result {
            Ok(()) => ThreadOutcome::Confirmed,
            Err(outcome) => outcome,
        });
        if store.finish(claim, &result).await.is_err() {
            if !budget.has_dispatched() {
                let _ = store.release(fence).await;
            }
            return Ok(receipt(ThreadOutcome::Unknown {
                reason: "topic_receipt_unconfirmed".into(),
            }));
        }
        if !matches!(result.outcome, ThreadOutcome::Unknown { .. })
            && store.release(fence).await.is_err()
        {
            tracing::warn!("topic mutation fence remains held after a completed receipt");
            if !budget.has_dispatched() {
                return Ok(receipt(ThreadOutcome::Unknown {
                    reason: "topic_cleanup_unconfirmed".into(),
                }));
            }
        }
        Ok(result)
    }
    async fn perform_topic_mutation(
        &self,
        caller: AgentCaller,
        channel: &ResolvedChannel,
        authority: &authority::Authority,
        action: Mutation<'_>,
        client: &TelegramTopics,
        budget: &Budget,
    ) -> Result<(), ThreadOutcome> {
        let credential = authority::token(channel, &authority.bot).ok_or_else(revoked)?;
        tokio::time::timeout_at(
            budget.preflight_end(),
            client
                .api
                .topic_permissions(credential, &authority.bot, &authority.chat),
        )
        .await
        .map_err(|_| rejected("topic_preflight_unavailable"))?
        .map_err(preflight_outcome)?;
        let current = tokio::time::timeout_at(
            budget.preflight_end(),
            self.recheck_topic_authority(caller, authority),
        )
        .await
        .map_err(|_| rejected("topic_preflight_unavailable"))?
        .map_err(|_| revoked())?;
        let credential = authority::token(&current, &authority.bot).ok_or_else(revoked)?;
        budget.dispatch()?;
        tokio::time::timeout_at(
            budget.write_end(),
            client.api.manage_topic(
                credential,
                &authority.bot,
                &authority.chat,
                &authority.topic,
                action,
            ),
        )
        .await
        .map_err(|_| ThreadOutcome::Unknown {
            reason: "telegram_topic_unconfirmed".into(),
        })?
        .map_err(write_outcome)
    }
}
fn rejected(reason: &str) -> ThreadOutcome {
    ThreadOutcome::Rejected {
        reason: reason.into(),
    }
}
fn preflight_outcome(error: TelegramError) -> ThreadOutcome {
    match error {
        TelegramError::RateLimited(retry_after) => ThreadOutcome::RateLimited { retry_after },
        TelegramError::Rejected => rejected("telegram_topic_permission_or_scope_denied"),
        TelegramError::Unconfirmed => rejected("telegram_topic_preflight_unavailable"),
    }
}
fn write_outcome(error: TelegramError) -> ThreadOutcome {
    match error {
        TelegramError::RateLimited(retry_after) => ThreadOutcome::RateLimited { retry_after },
        TelegramError::Rejected => rejected("telegram_topic_rejected"),
        TelegramError::Unconfirmed => ThreadOutcome::Unknown {
            reason: "telegram_topic_unconfirmed".into(),
        },
    }
}

#[cfg(test)]
mod tests;
