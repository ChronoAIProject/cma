//! Platform dispatch for the shared createEventThread entry point.
use super::*;
impl ToolsState {
    pub async fn create_event_thread(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        request: CreateThreadRequest,
        key: &str,
    ) -> Result<ThreadReceipt, Problem> {
        let event = reference
            .open(&self.keys, now())
            .map_err(|_| invalid_reference())?
            .payload;
        if matches!(event.reply, ReplyAddress::Telegram { .. }) {
            return self
                .create_telegram_topic(caller, event, request, key)
                .await;
        }
        let budget = Budget::new(self.thread_client()?.budget);
        tokio::time::timeout_at(
            budget.end,
            self.create_event_thread_inner(caller, reference, request, key, &budget),
        )
        .await
        .unwrap_or_else(|_| Ok(receipt(budget.expired())))
    }
    async fn create_event_thread_inner(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        request: CreateThreadRequest,
        key: &str,
        budget: &Budget,
    ) -> Result<ThreadReceipt, Problem> {
        super::super::validate_reply_key(Some(key))?;
        request
            .validate_discord()
            .map_err(|e| Problem::new(ProblemCode::InvalidRequest, 400, e.to_string()))?;
        let event = reference
            .open(&self.keys, now())
            .map_err(|_| invalid_reference())?
            .payload;
        let (channel, authority) = self.thread_authority(caller, &event).await?;
        let Some(target) = authority.origin.child(&authority.message) else {
            return Ok(receipt(ThreadOutcome::Unsupported {
                reason: "create_requires_original_guild_text_or_announcement_message".into(),
            }));
        };
        self.mutate_thread(
            caller,
            channel,
            authority,
            target,
            Operation::Create { request },
            key,
            budget,
        )
        .await
    }
}
