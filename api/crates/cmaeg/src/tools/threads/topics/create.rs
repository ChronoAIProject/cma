//! One new topic in the original admitted forum chat, with durable uncertain-create fencing.
use super::*;

impl ToolsState {
    pub(super) async fn topic_creation_authority(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
    ) -> Result<(Arc<ResolvedChannel>, authority::Authority), Problem> {
        if !self.topic_client()?.creation_enabled {
            return Err(Problem::new(
                ProblemCode::Forbidden,
                403,
                "topic_creation_disabled",
            ));
        }
        let current = self
            .resolver
            .resolve_current(event.channel_id.as_str())
            .await
            .map_err(|e| e.problem())?;
        super::super::super::state::owner(
            caller,
            &current,
            event.consumer.as_ref(),
            "createEventThread",
        )?;
        if !self.resolver.record_client().thread_fences_ready().await {
            return Err(unavailable());
        }
        authority::load(self, caller, event, &current, true).await
    }
    pub(in crate::tools) async fn topic_creation_capability(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
    ) -> Option<bool> {
        let client = self.telegram_topics.as_deref()?;
        if !client.creation_enabled || !matches!(event.reply, ReplyAddress::Telegram { .. }) {
            return None;
        }
        let _permit = client.slots.try_acquire().ok()?;
        tokio::time::timeout(
            budget::DISCOVERY,
            self.topic_creation_authority(caller, event),
        )
        .await
        .ok()?
        .ok()?;
        Some(true)
    }
    pub(in crate::tools::threads) async fn create_telegram_topic(
        &self,
        caller: AgentCaller,
        event: EventRefPayload,
        request: CreateThreadRequest,
        key: &str,
    ) -> Result<ThreadReceipt, Problem> {
        if !self.topic_client()?.creation_enabled {
            return Err(Problem::new(
                ProblemCode::Forbidden,
                403,
                "topic_creation_disabled",
            ));
        }
        super::super::super::validate_reply_key(Some(key))?;
        request
            .validate_telegram()
            .map_err(|e| Problem::new(ProblemCode::InvalidRequest, 400, e.to_string()))?;
        let budget = Budget::new(self.topic_client()?.budget);
        tokio::time::timeout_at(
            budget.end,
            self.create_topic_inner(caller, event, request, key, &budget),
        )
        .await
        .unwrap_or_else(|_| Ok(receipt(budget.expired())))
    }
    async fn create_topic_inner(
        &self,
        caller: AgentCaller,
        event: EventRefPayload,
        request: CreateThreadRequest,
        key: &str,
        budget: &Budget,
    ) -> Result<ThreadReceipt, Problem> {
        let client = self.topic_client()?;
        let _permit = client.slots.try_acquire().map_err(|_| busy())?;
        let (channel, authority) = self.topic_creation_authority(caller, &event).await?;
        let store = ledger::Store::telegram(self.resolver.record_client(), self.keys.clone());
        let scope = hash(
            &serde_json::json!({"operation":"create_topic","event":event,"bot":authority.bot,"chat":authority.chat}),
        );
        budget.start_ledger();
        let claim = match store
            .claim(
                scope,
                key,
                hash(&request),
                &channel.definition.owner.subject,
                event.expires_at,
            )
            .await?
        {
            ledger::Admission::Replay(saved) => return Ok(saved),
            ledger::Admission::Claimed(claim) => claim,
        };
        let Some(fence) = store
            .acquire_topic_creation(
                &authority.bot,
                &authority.chat,
                &channel.definition.owner.subject,
            )
            .await?
        else {
            let result = receipt(ThreadOutcome::Unknown {
                reason: "topic_creation_in_progress_or_uncertain".into(),
            });
            store.finish(claim, &result).await?;
            return Ok(result);
        };
        let result = match self
            .perform_topic_creation(caller, &channel, &authority, &request, client, budget)
            .await
        {
            Ok(topic) => {
                // Confirmation does not grant a new reference after custody was revoked in flight.
                let thread = if self
                    .recheck_topic_authority(caller, &authority)
                    .await
                    .is_ok()
                {
                    self.created_topic_summary(&authority, &topic).ok()
                } else {
                    None
                };
                ThreadReceipt {
                    outcome: ThreadOutcome::Confirmed,
                    thread,
                }
            }
            Err(outcome) => receipt(outcome),
        };
        if store.finish(claim, &result).await.is_err() {
            if !budget.has_dispatched() {
                let _ = store.release(fence).await;
            }
            return Ok(receipt(ThreadOutcome::Unknown {
                reason: "topic_creation_receipt_unconfirmed".into(),
            }));
        }
        if !matches!(result.outcome, ThreadOutcome::Unknown { .. })
            && store.release(fence).await.is_err()
        {
            tracing::warn!("topic creation fence remains held after a completed receipt");
            if !budget.has_dispatched() {
                return Ok(receipt(ThreadOutcome::Unknown {
                    reason: "topic_creation_cleanup_unconfirmed".into(),
                }));
            }
        }
        Ok(result)
    }
    async fn perform_topic_creation(
        &self,
        caller: AgentCaller,
        channel: &ResolvedChannel,
        authority: &authority::Authority,
        request: &CreateThreadRequest,
        client: &TelegramTopics,
        budget: &Budget,
    ) -> Result<crate::telegram::topics::CreatedTopic, ThreadOutcome> {
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
            client
                .api
                .create_topic(credential, &authority.bot, &authority.chat, &request.name),
        )
        .await
        .map_err(|_| ThreadOutcome::Unknown {
            reason: "telegram_topic_creation_unconfirmed".into(),
        })?
        .map_err(write_outcome)
    }
    fn created_topic_summary(
        &self,
        authority: &authority::Authority,
        topic: &crate::telegram::topics::CreatedTopic,
    ) -> Result<ThreadSummary, Problem> {
        let payload = TelegramThreadRefPayload {
            v: 2,
            event: authority.event.clone(),
            custody: authority.custody.clone(),
            chat_id: authority.chat.clone(),
            thread_id: topic.id.clone(),
            issued_at: now(),
            expires_at: authority.event.expires_at,
        };
        Ok(ThreadSummary {
            thread_ref: ThreadRef::seal_telegram(&payload, &self.keys)
                .map_err(|_| unavailable())?,
            thread_id: topic.id.clone(),
            parent_id: authority.chat.clone(),
            name: topic.name.clone(),
            // A newly created Telegram topic is open; neither Discord state is asserted true.
            archived: false,
            locked: false,
            expires_at: payload.expires_at,
        })
    }
}
