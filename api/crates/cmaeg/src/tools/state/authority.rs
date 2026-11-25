//! Existing tool authority checks precede diagnostic header interpretation.
use super::*;

impl ToolsState {
    /// Open `reference` and authorize `caller` as the consumer it was issued
    /// to, returning that consumer with the channel and event. `tool` names
    /// the operation in refusal logs.
    pub async fn authorize_event(
        &self,
        caller: AgentCaller,
        reference: &EventRef,
        tool: &'static str,
    ) -> Result<(Arc<ResolvedChannel>, EventRefPayload, ConsumerId), Problem> {
        let event = reference
            .open(&self.keys, now())
            .map_err(open_error)?
            .payload;
        let channel = self
            .resolver
            .resolve(event.channel_id.as_str())
            .await
            .map_err(|error| error.problem())?;
        let channel = if channel.definition.secrets.profile_revisions.is_some() {
            let fresh = self
                .resolver
                .resolve_current(event.channel_id.as_str())
                .await
                .map_err(|error| error.problem())?;
            crate::channels::profile_assignment::event_channel(
                &self.resolver,
                fresh,
                &event.event_id,
                event.consumer.as_ref(),
                Some(&event.conversation_id),
            )
            .await
            .map_err(|error| error.problem())?
        } else {
            channel
        };
        let matched = owner(caller, &channel, event.consumer.as_ref(), tool)?;
        if event.record_id != channel.pointer.record_id {
            return Err(invalid("event channel does not match its record"));
        }
        if tool == "replyToEvent" {
            crate::telemetry::turn_trace::ingress::authorized();
        }
        Ok((channel, event, matched))
    }
    /// Open `reference` and authorize `caller` as the consumer whose message
    /// it names, as [`ToolsState::authorize_event`] does for events.
    pub async fn authorize_message(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        tool: &'static str,
    ) -> Result<(Arc<ResolvedChannel>, MessageRefPayload, ConsumerId), Problem> {
        let message = reference
            .open(&self.keys)
            .map_err(|_| invalid("message_ref_invalid"))?
            .payload;
        let channel = if message.native_media_edit.is_some()
            || message.received_reaction.is_some()
            || crate::interactions::native_lark::is_reference(&message)
            || message.native_lark_effect.is_some()
            || message.whatsapp_reaction.is_some()
            || message.lark_effect.is_some()
            || message.discord_message.is_some()
            || (message.interaction_record_id.is_some() && message.route_id.starts_with("discord:"))
        {
            self.resolver
                .resolve_current(message.channel_id.as_str())
                .await
        } else {
            self.resolver.resolve(message.channel_id.as_str()).await
        }
        .map_err(|error| error.problem())?;
        let channel = if channel.definition.secrets.profile_revisions.is_some() {
            let fresh = self
                .resolver
                .resolve_current(message.channel_id.as_str())
                .await
                .map_err(|error| error.problem())?;
            crate::channels::profile_assignment::event_channel(
                &self.resolver,
                fresh,
                &message.event_id,
                message.consumer.as_ref(),
                None,
            )
            .await
            .map_err(|error| error.problem())?
        } else {
            channel
        };
        let matched = owner(caller, &channel, message.consumer.as_ref(), tool)?;
        if tool == "updateMessage" {
            crate::telemetry::turn_trace::ingress::authorized();
        }
        Ok((channel, message, matched))
    }
}
