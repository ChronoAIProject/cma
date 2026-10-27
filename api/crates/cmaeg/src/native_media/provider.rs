//! Provider preparation and one write; durable authority transitions live in execute.
use super::{
    model::{Current, Observation, Original},
    Client,
};
use crate::{
    discord::rest::{media_edit as discord, RestError},
    media::outbound::PreparedFile,
    telegram::{media_edit as telegram, Error},
};
use cmaeg_core::{
    channel::Secret,
    message::NativeMediaUpdate,
    seal::{native_media_edit::NativeMediaDestination as Destination, MessageRefPayload},
};

pub(super) enum Prepared {
    Telegram(telegram::PreparedEdit),
    Discord(discord::PreparedEdit),
}
pub(super) enum PreparationError {
    Rejected(&'static str),
    Changed,
}
pub(super) enum WriteError {
    Rejected,
    RateLimited(u64),
    Unknown,
}
pub(super) async fn prepare(
    client: &Client,
    token: &Secret,
    message: &MessageRefPayload,
    original: &Original,
    current: &Current,
    patch: &NativeMediaUpdate,
    file: Option<PreparedFile>,
) -> Result<Prepared, PreparationError> {
    let a = message
        .native_media_edit
        .as_ref()
        .expect("authorized media reference");
    match &a.destination {
        Destination::Telegram {
            bot_id, chat_id, ..
        } => {
            if client
                .telegram
                .get_me(token)
                .await
                .ok()
                .is_none_or(|b| b.id != *bot_id)
            {
                return Err(PreparationError::Rejected(
                    "native_media_bot_custody_unavailable",
                ));
            }
            client
                .telegram
                .prepare_media_edit(
                    chat_id,
                    &message.platform_message_id,
                    &current.snapshot,
                    patch,
                    file,
                )
                .await
                .map(Prepared::Telegram)
                .map_err(|_| PreparationError::Rejected("native_media_request_invalid"))
        }
        Destination::Discord { bot_id, channel_id } => {
            if client
                .discord
                .verify_media_bot(token, bot_id)
                .await
                .is_err()
            {
                return Err(PreparationError::Rejected(
                    "native_media_bot_custody_unavailable",
                ));
            }
            let channel = client
                .discord
                .media_channel(token, channel_id, &original.activity.conversation)
                .await
                .map_err(|error| match error {
                    RestError::NotFound | RestError::Rejected => PreparationError::Changed,
                    _ => PreparationError::Rejected("native_media_channel_unavailable"),
                })?;
            if Some(&channel) != original.discord_channel.as_ref() {
                return Err(PreparationError::Changed);
            }
            let body = client
                .discord
                .message(token, channel_id, &message.platform_message_id)
                .await
                .map_err(|error| match error {
                    RestError::NotFound | RestError::Rejected => PreparationError::Changed,
                    _ => PreparationError::Rejected("native_media_message_unavailable"),
                })?;
            let observed = discord::observe(
                &body,
                bot_id,
                channel_id,
                &message.platform_message_id,
                a.kind,
            )
            .map_err(|_| PreparationError::Changed)?;
            if !observed.eligible
                || observed.snapshot.for_revision(&current.snapshot).as_ref()
                    != Some(&current.snapshot)
            {
                return Err(PreparationError::Changed);
            }
            client
                .discord
                .prepare_media_edit(patch, &current.snapshot, file)
                .await
                .map(Prepared::Discord)
                .map_err(|_| PreparationError::Rejected("native_media_request_invalid"))
        }
    }
}
impl Prepared {
    pub(super) async fn send(
        self,
        client: &Client,
        token: &Secret,
        message: &MessageRefPayload,
    ) -> Result<Observation, WriteError> {
        let a = message
            .native_media_edit
            .as_ref()
            .expect("authorized media reference");
        match (self, &a.destination) {
            (
                Self::Telegram(edit),
                Destination::Telegram {
                    bot_id,
                    chat_id,
                    thread_id,
                },
            ) => {
                let body = client
                    .telegram
                    .edit_media(token, edit)
                    .await
                    .map_err(|error| match error {
                        Error::Rejected => WriteError::Rejected,
                        Error::RateLimited(seconds) => WriteError::RateLimited(seconds),
                        _ => WriteError::Unknown,
                    })?;
                telegram::observe(
                    &body,
                    bot_id,
                    chat_id,
                    thread_id.as_deref(),
                    &message.platform_message_id,
                    a.kind,
                )
                .map_err(|_| WriteError::Unknown)
            }
            (Self::Discord(edit), Destination::Discord { bot_id, channel_id }) => {
                let body = client
                    .discord
                    .edit_media(token, channel_id, &message.platform_message_id, edit)
                    .await
                    .map_err(|error| match error {
                        RestError::Rejected | RestError::NotFound | RestError::Unavailable => {
                            WriteError::Rejected
                        }
                        RestError::RateLimited(seconds) => WriteError::RateLimited(seconds),
                        _ => WriteError::Unknown,
                    })?;
                discord::observe(
                    &body,
                    bot_id,
                    channel_id,
                    &message.platform_message_id,
                    a.kind,
                )
                .map_err(|_| WriteError::Unknown)
            }
            _ => Err(WriteError::Unknown),
        }
    }
}
