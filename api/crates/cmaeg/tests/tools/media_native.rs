//! Native fixtures retain the same source and destination in activity and seal.
use cmaeg_core::{
    activity::{Activity, ConversationKind, SourceKind},
    channel::{ChannelDefinition, Platform, Source},
    seal::ReplyAddress,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub enum NativeMediaPlatform {
    Telegram,
    Discord,
    Whatsapp,
    WhatsappGroup,
}

impl NativeMediaPlatform {
    pub fn retarget(self, activity: &mut Activity) {
        activity.conversation.thread_id = None;
        match self {
            Self::Telegram => {
                activity.source = SourceKind::Telegram {
                    platform: Platform::Telegram,
                    bot_id: "77".into(),
                };
            }
            Self::Discord => {
                activity.source = SourceKind::Discord {
                    bot_id: "77".into(),
                };
                activity.conversation.id = "99".into();
            }
            Self::Whatsapp | Self::WhatsappGroup => {
                activity.source = SourceKind::Whatsapp {
                    app_id: "12345".into(),
                    waba_id: "23456".into(),
                    phone_number_id: "34567".into(),
                };
                activity.actor.id = "447700900123".into();
                let group = matches!(self, Self::WhatsappGroup);
                activity.conversation.id = if group { "group-1" } else { "447700900123" }.into();
                activity.conversation.kind = if group {
                    ConversationKind::Group
                } else {
                    ConversationKind::Private
                };
            }
        }
    }

    pub fn reply(self, edit: fn(&mut Value)) -> ReplyAddress {
        match self {
            Self::Telegram => ReplyAddress::Telegram {
                bot_id: "77".into(),
                chat_id: "42".into(),
                message_id: "5".into(),
                thread_id: None,
            },
            Self::Discord => ReplyAddress::Discord {
                bot_id: "77".into(),
                channel_id: "99".into(),
                message_id: "700".into(),
                thread_id: None,
            },
            Self::Whatsapp => ReplyAddress::Whatsapp {
                app_id: "12345".into(),
                waba_id: "23456".into(),
                phone_number_id: "34567".into(),
                recipient_id: "447700900123".into(),
                message_id: "wamid.original".into(),
            },
            Self::WhatsappGroup => {
                // The fixture must use the same source and credential revision
                // as ingress, so live custody checks run during redemption.
                let mut value: Value = serde_json::from_str(include_str!(
                    "../../../cmaeg-core/tests/fixtures/channel.json"
                ))
                .unwrap();
                edit(&mut value);
                let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
                let source = definition
                    .sources
                    .iter()
                    .find(|source| matches!(source, Source::Whatsapp { .. }))
                    .unwrap();
                let credentials = definition.secrets.whatsapp.unwrap();
                let revision = format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(&(
                            "native_whatsapp_group_revision_v1",
                            source,
                            credentials.app_secret.as_str(),
                            credentials.access_token.as_str(),
                            credentials.verify_token.as_str(),
                        ))
                        .unwrap()
                    )
                );
                ReplyAddress::WhatsappGroup(cmaeg_core::channel::whatsapp::WhatsappGroupReply {
                    revision,
                    app_id: "12345".into(),
                    waba_id: "23456".into(),
                    phone_number_id: "34567".into(),
                    group_id: "group-1".into(),
                    participant_id: "447700900123".into(),
                    message_id: "wamid.original".into(),
                })
            }
        }
    }
}
