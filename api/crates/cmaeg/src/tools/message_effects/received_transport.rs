//! Provider writes retain received-message scope and recheck custody after reads.
use super::{received_authority, ToolsState};
use crate::{
    auth::AgentCaller, channels::resolver::ResolvedChannel, delivery::intents::DeliveryOutcome,
    discord::RestError, telegram::Error,
};
use cmaeg_core::{
    ids::MessageRef,
    message_effect::MessageReactionRequest,
    seal::{received_reaction::ReceivedReactionPlatform as Platform, MessageRefPayload},
};

impl ToolsState {
    pub(super) async fn send_received_reaction(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        channel: &ResolvedChannel,
        message: &MessageRefPayload,
        request: &MessageReactionRequest,
    ) -> DeliveryOutcome {
        let a = message
            .received_reaction
            .as_ref()
            .expect("checked received scope");
        let result = match a.platform {
            Platform::Telegram => {
                let client = self.telegram_effects.as_ref().expect("checked client");
                let token = channel
                    .definition
                    .secrets
                    .telegram_bot_token
                    .as_ref()
                    .expect("checked token");
                client
                    .telegram
                    .set_message_reaction(token, &a.chat_id, &message.platform_message_id, request)
                    .await
                    .map_err(|e| match e {
                        Error::Rejected => RestError::Rejected,
                        Error::RateLimited(delay) => RestError::RateLimited(delay),
                        Error::Unconfirmed => RestError::Unknown,
                    })
            }
            Platform::Discord => {
                let client = self.discord_effects.as_ref().expect("checked client");
                let token = crate::discord::rest::token(channel, &a.bot_id).expect("checked token");
                client
                    .api
                    .set_received_message_reaction(
                        token,
                        &a.bot_id,
                        &a.chat_id,
                        &message.platform_message_id,
                        &a.actor_id,
                        request,
                        || async {
                            let (current, scope) = self
                                .current_message(caller, reference)
                                .await
                                .map_err(|_| RestError::Rejected)?;
                            received_authority::check(&scope, &current, caller)
                                .map_err(|_| RestError::Rejected)
                        },
                    )
                    .await
            }
        };
        match result {
            Ok(()) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(message.platform_message_id.clone()),
            },
            Err(RestError::RateLimited(retry_after)) => {
                DeliveryOutcome::RateLimited { retry_after }
            }
            Err(RestError::Unknown) => DeliveryOutcome::Unknown {
                reason: "received_reaction_unconfirmed".into(),
            },
            Err(RestError::Unavailable) => DeliveryOutcome::Rejected {
                reason: "received_reaction_unavailable".into(),
            },
            Err(RestError::Rejected | RestError::NotFound) => DeliveryOutcome::Rejected {
                reason: "received_reaction_rejected".into(),
            },
        }
    }
}
