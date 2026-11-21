//! Native typing uses the existing operation with immutable received-message scope.
mod authority;
use super::ToolsState;
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::intents::{TypingOutcome, TypingReceipt},
    discord::{DiscordRest, RestError},
};
use cmaeg_core::{
    activity::Activity,
    problem::{Problem, ProblemCode},
    seal::{EventRefPayload, ReplyAddress},
};
use std::time::Duration;
use tokio::sync::Semaphore;

pub struct NativeTyping {
    whatsapp: crate::whatsapp::Client,
    discord: DiscordRest,
    whatsapp_enabled: bool,
    discord_enabled: bool,
    capacity: Semaphore,
}
impl NativeTyping {
    pub fn new(
        whatsapp: crate::whatsapp::Client,
        discord: DiscordRest,
        whatsapp_enabled: bool,
        discord_enabled: bool,
    ) -> Self {
        Self {
            whatsapp,
            discord,
            whatsapp_enabled,
            discord_enabled,
            capacity: Semaphore::new(32),
        }
    }
    fn enabled(&self, event: &EventRefPayload) -> bool {
        match event.reply {
            ReplyAddress::Whatsapp { .. } => self.whatsapp_enabled,
            ReplyAddress::Discord { .. } => self.discord_enabled,
            _ => false,
        }
    }
}

impl ToolsState {
    pub(super) async fn native_typing_capability(
        &self,
        caller: AgentCaller,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        verified: bool,
    ) -> bool {
        if !verified
            || !self
                .native_typing
                .as_ref()
                .is_some_and(|client| client.enabled(event))
        {
            return false;
        }
        matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                authority::load(self, caller, channel, event, None)
            )
            .await,
            Ok(Ok(_))
        )
    }

    pub(super) async fn send_native_typing(
        &self,
        caller: AgentCaller,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
    ) -> Result<TypingReceipt, Problem> {
        let Some(client) = self
            .native_typing
            .as_ref()
            .filter(|client| client.enabled(event))
        else {
            return Ok(TypingReceipt {
                outcome: TypingOutcome::Unsupported,
            });
        };
        let _permit = client
            .capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "native_typing_capacity"))?;
        let remaining = i128::from(event.expires_at) * 1_000_000_000
            - time::OffsetDateTime::now_utc().unix_timestamp_nanos();
        if remaining <= 0 {
            return Err(Problem::new(
                ProblemCode::EventRefExpired,
                410,
                "event_ref_expired",
            ));
        }
        let budget = Duration::from_nanos(remaining.min(7_000_000_000) as u64);
        tokio::time::timeout(budget, async {
            let authorized = authority::load(self, caller, channel, event, None).await?;
            match &event.reply {
                ReplyAddress::Whatsapp {
                    phone_number_id,
                    message_id,
                    ..
                } => {
                    let source = authorized
                        .channel
                        .definition
                        .source_of(&authorized.activity.source)
                        .ok_or_else(authority::denied)?;
                    let credentials =
                        crate::whatsapp::credentials(&authorized.channel.definition, &event.reply)
                            .ok_or_else(authority::denied)?;
                    client
                        .whatsapp
                        .verify_custody(source, credentials)
                        .await
                        .map_err(whatsapp_problem)?;
                    let current = self
                        .before_native_typing(caller, event, &authorized)
                        .await?;
                    let credentials =
                        crate::whatsapp::credentials(&current.definition, &event.reply)
                            .ok_or_else(authority::denied)?;
                    client
                        .whatsapp
                        .typing(credentials, phone_number_id, message_id)
                        .await
                        .map_err(whatsapp_problem)?;
                }
                ReplyAddress::Discord {
                    bot_id,
                    channel_id,
                    message_id,
                    ..
                } => {
                    let token = crate::discord::rest::token(&authorized.channel, bot_id)
                        .ok_or_else(authority::denied)?;
                    client
                        .discord
                        .typing(
                            token,
                            bot_id,
                            channel_id,
                            message_id,
                            &authorized.activity.actor.id,
                            || async {
                                self.before_native_typing(caller, event, &authorized)
                                    .await
                                    .map(|_| ())
                                    .map_err(|error| {
                                        if error.status >= 500 {
                                            RestError::Unavailable
                                        } else {
                                            RestError::Rejected
                                        }
                                    })
                            },
                        )
                        .await
                        .map_err(discord_problem)?;
                }
                _ => return Err(authority::denied()),
            }
            Ok(TypingReceipt {
                outcome: TypingOutcome::Sent,
            })
        })
        .await
        .map_err(|_| unavailable())?
    }

    async fn before_native_typing(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        original: &authority::Authorized,
    ) -> Result<std::sync::Arc<ResolvedChannel>, Problem> {
        let current = authority::load(
            self,
            caller,
            &original.channel,
            event,
            Some(&original.activity),
        )
        .await?;
        if current.custody != original.custody {
            return Err(authority::denied());
        }
        Ok(current.channel)
    }
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "native_typing_acceptance_unconfirmed",
    )
}
fn whatsapp_problem(error: crate::whatsapp::Error) -> Problem {
    match error {
        crate::whatsapp::Error::RateLimited => {
            Problem::new(ProblemCode::RateLimited, 429, "native_typing_rate_limited")
        }
        crate::whatsapp::Error::Unconfirmed => unavailable(),
        _ => authority::denied(),
    }
}
fn discord_problem(error: RestError) -> Problem {
    match error {
        RestError::RateLimited(_) => {
            Problem::new(ProblemCode::RateLimited, 429, "native_typing_rate_limited")
        }
        RestError::Unknown | RestError::Unavailable => unavailable(),
        _ => authority::denied(),
    }
}

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod transport_tests;
