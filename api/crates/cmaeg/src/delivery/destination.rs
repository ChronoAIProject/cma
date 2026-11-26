//! Nonsecret destination authority pinned by a routed turn, never credentials.
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::channel::{DefaultReply, Platform, Sink, Source, SourceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum DefaultDestination {
    Telegram {
        source_id: SourceId,
        bot_id: String,
        chat_id: String,
        thread_id: Option<String>,
    },
    Relay {
        source_id: SourceId,
        platform: Platform,
        route_id: String,
    },
    Webhooks {
        sinks: Vec<WebhookDestination>,
    },
}
pub use cmaeg_engine::WebhookDestination;
impl DefaultDestination {
    /// Called during initial preparation. A restored turn retains these exact
    /// nonsecret addresses, while every send uses freshly resolved credentials.
    pub fn capture(channel: &ResolvedChannel) -> Option<Self> {
        let definition = &channel.definition;
        let policy = definition.routing.as_ref()?.default_reply.as_ref()?;
        let source = |id: &SourceId| definition.sources.iter().find(|s| s.source_id() == *id);
        match policy {
            DefaultReply::TelegramChat {
                source_id,
                chat_id,
                thread_id,
            } => {
                let Source::Telegram { bot_id, .. } = source(source_id)? else {
                    return None;
                };
                definition.secrets.telegram_bot_token.as_ref()?;
                Some(Self::Telegram {
                    source_id: source_id.clone(),
                    bot_id: bot_id.clone(),
                    chat_id: chat_id.clone(),
                    thread_id: thread_id.clone(),
                })
            }
            DefaultReply::RelayConversation {
                source_id,
                route_id,
            } => {
                let Source::NyxidRelay {
                    platform,
                    route_ids,
                    ..
                } = source(source_id)?
                else {
                    return None;
                };
                definition.secrets.channel_key.as_ref()?;
                route_ids.contains(route_id).then(|| Self::Relay {
                    source_id: source_id.clone(),
                    platform: *platform,
                    route_id: route_id.clone(),
                })
            }
            DefaultReply::Sinks {} => {
                let sinks: Vec<_> = definition
                    .reply
                    .sinks
                    .iter()
                    .filter_map(|sink| match sink {
                        Sink::ResultWebhook { url, hmac_key_id } => Some(WebhookDestination {
                            url: url.clone(),
                            hmac_key_id: hmac_key_id.clone(),
                        }),
                        _ => None,
                    })
                    .collect();
                if sinks.is_empty()
                    || sinks.iter().any(|sink| {
                        !definition
                            .secrets
                            .hmac_secrets
                            .iter()
                            .any(|(id, _)| id == &sink.hmac_key_id)
                    })
                {
                    return None;
                }
                Some(Self::Webhooks { sinks })
            }
        }
    }
    pub fn authorized(&self, channel: &ResolvedChannel) -> bool {
        Self::capture(channel).as_ref() == Some(self)
    }
}
