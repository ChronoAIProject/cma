//! What the channel detail tabs show, projected from a definition when its
//! summary is built and cached with it, so a tab render reads no CMA while
//! the version is unchanged.
//!
//! The projection keeps settings, counts and whether each secret is set;
//! never a secret, a profile, a full key hash, a sender or chat id, a
//! result webhook URL (its query may carry a token) or a connection
//! reference.
use cmaeg_core::channel::{
    AdmissionPolicy, ChannelDefinition, ConformanceLevel, CustomAuth, GroupAdmission,
    IdentityAdmission, PartitionKey, ProviderTarget, Sink, Source, SourceId,
};
use serde::Serialize;

use super::{mask, routing_view::RoutingView};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelDetail {
    pub partition: &'static str,
    /// The key custom events are partitioned by, when it differs from
    /// `partition` (`custom_key`); `custom_key_inert` when no custom source
    /// is attached to use it.
    pub custom_key: Option<&'static str>,
    pub custom_key_inert: bool,
    pub reply: ReplyView,
    pub sources: Vec<SourceView>,
    pub secrets: SecretsView,
    /// True when a connection manages the channel's sources.
    pub connection_managed: bool,
    /// The routing policy; absent on a legacy channel.
    pub routing: Option<RoutingView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplyView {
    pub auto_final: bool,
    pub ack_after_ms: u32,
    /// `source`, `sync` or `result_webhook (key <id>)`.
    pub sinks: Vec<String>,
}

/// One source, as the Sources tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceView {
    /// The source's slot, which its activity entries also carry.
    pub id: SourceId,
    /// `telegram`, `nyxid_relay` or `custom`.
    pub kind: &'static str,
    pub platform: Option<&'static str>,
    pub route_count: Option<usize>,
    pub bot_username: Option<String>,
    pub bot_id: Option<String>,
    pub admission: Option<String>,
    /// How its events authenticate, with the key ids that may sign them.
    pub auth: String,
    pub key_ids: Vec<String>,
    /// Further settings, such as attachments or a prompt template.
    pub settings: Vec<String>,
}

/// Whether each secret is set; never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretsView {
    /// The agent key's hash as `sha256:…<last 8>`.
    pub agent_key_hash: String,
    /// Whether the default consumer's agent key is held.
    pub agent_key: bool,
    pub channel_key: bool,
    pub telegram_bot_token: bool,
    pub telegram_webhook_secret: bool,
    /// The key ids that have an HMAC secret.
    pub hmac_key_ids: Vec<String>,
}

impl ChannelDetail {
    pub fn of(definition: &ChannelDefinition) -> Self {
        let secrets = &definition.secrets;
        Self {
            partition: partition_label(definition.partition.key),
            custom_key: definition.partition.custom_key.map(partition_label),
            custom_key_inert: definition.partition.custom_key.is_some()
                && !definition
                    .sources
                    .iter()
                    .any(|source| matches!(source, Source::Custom { .. })),
            reply: ReplyView {
                auto_final: definition.reply.auto_final,
                ack_after_ms: definition.reply.ack_after_ms,
                sinks: definition.reply.sinks.iter().map(sink_label).collect(),
            },
            sources: definition.sources.iter().map(source_view).collect(),
            secrets: SecretsView {
                agent_key_hash: mask::hash(&secrets.agent_key_hash),
                agent_key: definition
                    .consumer(None)
                    .is_some_and(|consumer| consumer.credential.is_some()),
                channel_key: secrets.channel_key.is_some(),
                telegram_bot_token: secrets.telegram_bot_token.is_some(),
                telegram_webhook_secret: secrets.telegram_webhook_secret.is_some(),
                hmac_key_ids: secrets
                    .hmac_secrets
                    .iter()
                    .map(|(key_id, _)| key_id.clone())
                    .collect(),
            },
            connection_managed: definition.connection_ref.is_some(),
            routing: RoutingView::of(definition),
        }
    }
}

/// A provider target as the Consumers tab shows it.
pub fn target_label(target: &ProviderTarget) -> String {
    match target {
        ProviderTarget::Profile {} => "profile".to_owned(),
        ProviderTarget::ExistingAgent { agent_id } => format!("existing agent {agent_id}"),
    }
}

pub fn level_label(level: ConformanceLevel) -> &'static str {
    match level {
        ConformanceLevel::L1 => "L1",
        ConformanceLevel::L2 => "L2",
        ConformanceLevel::L3 => "L3",
        ConformanceLevel::L4 => "L4",
        ConformanceLevel::L5 => "L5",
    }
}

fn partition_label(key: PartitionKey) -> &'static str {
    match key {
        PartitionKey::ConversationAndSender => "conversation_and_sender",
        PartitionKey::Conversation => "conversation",
        PartitionKey::Thread => "thread",
        PartitionKey::Fixed => "fixed",
        PartitionKey::EventSubject => "event_subject",
        PartitionKey::PerEvent => "per_event",
    }
}

fn sink_label(sink: &Sink) -> String {
    match sink {
        Sink::Source {} => "source".to_owned(),
        Sink::Sync {} => "sync".to_owned(),
        Sink::ResultWebhook { hmac_key_id, .. } => format!("result_webhook (key {hmac_key_id})"),
    }
}

fn source_view(source: &Source) -> SourceView {
    let id = source.source_id();
    match source {
        Source::NativeLark(source) => SourceView {
            id,
            kind: "Native Lark / Feishu",
            platform: Some(source.identity.region.as_str()),
            route_count: None,
            bot_username: None,
            bot_id: Some(source.identity.bot_open_id.clone()),
            admission: Some(admission_label(&source.admission)),
            auth: "Encrypted and signed native app callback".into(),
            key_ids: Vec::new(),
            settings: vec![
                format!(
                    "Separate app {}; tenant {}. Do not replace an existing NyxID app webhook.",
                    source.identity.app_id, source.identity.tenant_key
                ),
                "Text, recall and human reaction deltas for retained messages; incoming edits unavailable.".into(),
            ],
        },
        Source::Whatsapp {
            app_id,
            waba_id,
            phone_number_id,
            group_ids,
            forms_enabled,
            message_events,
            admission,
        } => SourceView {
            id,
            kind: "Native WhatsApp",
            platform: Some("whatsapp"),
            route_count: None,
            bot_username: None,
            bot_id: Some(phone_number_id.clone()),
            admission: Some(admission_label(admission)),
            auth: "Meta app signature".into(),
            key_ids: Vec::new(),
            settings: vec![
                format!("App {app_id}; account {waba_id}; dedicated callback required"),
                format!("Managed static form consent: {forms_enabled}"),
                format!("Coexistence incoming edit/deletion consent: {message_events}"),
                format!("Private messages and reactions; {} explicitly allowed existing groups. Media and templates require separate gateway capabilities.",group_ids.len()),
            ],
        },
        Source::Discord {
            bot_id, admission, ..
        } => SourceView {
            id,
            kind: "discord",
            platform: Some("discord"),
            route_count: None,
            bot_username: None,
            bot_id: Some(bot_id.clone()),
            admission: Some(admission_label(admission)),
            auth: "Discord bot token".into(),
            key_ids: Vec::new(),
            settings: vec!["Gateway intake not activated".into()],
        },
        Source::Telegram {
            topic_lifecycle: _,
            topic_creation: _,
            message_events: _,
            channel_posts: _,
            member_join_chat_ids: _,
            bot_id,
            bot_username,
            admission,
        } => SourceView {
            id,
            kind: "telegram",
            platform: Some("telegram"),
            route_count: None,
            bot_username: bot_username.clone(),
            bot_id: Some(bot_id.clone()),
            admission: Some(admission_label(admission)),
            auth: "Telegram webhook secret".to_owned(),
            key_ids: Vec::new(),
            settings: Vec::new(),
        },
        Source::NyxidRelay {
            issuer,
            key_id,
            route_ids,
            platform,
            bot_username,
            bot_id,
            trust_normalized,
            verify_raw_event,
            admission,
            ..
        } => SourceView {
            id,
            kind: "nyxid_relay",
            platform: Some(platform.as_str()),
            route_count: Some(route_ids.len()),
            bot_username: bot_username.clone(),
            bot_id: bot_id.clone(),
            admission: Some(admission_label(admission)),
            auth: format!("NyxID relay token from {issuer}"),
            key_ids: vec![key_id.clone()],
            settings: trust_normalized
                .then(|| "trusts normalized events".to_owned())
                .into_iter()
                .chain(verify_raw_event.then(|| "verifies raw platform events".to_owned()))
                .collect(),
        },
        Source::Custom {
            auth,
            prompt_template,
            schema,
            attachments,
        } => {
            let (auth, key_ids) = match auth {
                CustomAuth::Hmac { key_id } => ("HMAC signature".to_owned(), vec![key_id.clone()]),
                CustomAuth::NyxidKey { key_hashes } => (
                    "NyxID key".to_owned(),
                    key_hashes.iter().map(|hash| mask::hash(hash)).collect(),
                ),
            };
            let mut settings = Vec::new();
            if prompt_template.is_some() {
                settings.push("prompt template".to_owned());
            }
            if schema.is_some() {
                settings.push("event schema".to_owned());
            }
            if let Some(attachments) = attachments {
                settings.push(format!(
                    "attachments at {} from {} origin(s)",
                    attachments.pointer,
                    attachments.origins.len()
                ));
            }
            SourceView {
                id,
                kind: "custom",
                platform: None,
                route_count: None,
                bot_username: None,
                bot_id: None,
                admission: None,
                auth,
                key_ids,
                settings,
            }
        }
        Source::Github {
            app_ref,
            installation_ids,
            repository_ids,
            deny_repository_ids,
            events,
            actions,
            prompt_template,
        } => SourceView {
            id,
            kind: "github",
            platform: Some("github"),
            route_count: Some(repository_ids.len()),
            bot_username: None,
            bot_id: None,
            admission: None,
            auth: "GitHub App installation binding".into(),
            key_ids: Vec::new(),
            settings: vec![
                format!("App {app_ref}; {} installation(s)", installation_ids.len()),
                format!("{} repository allowlist item(s), {} deny item(s)", repository_ids.len(), deny_repository_ids.len()),
                format!("Events: {}; actions: {}", events.join(","), actions.join(",")),
                if prompt_template.is_some() { "prompt template".into() } else { "default event rendering".into() },
            ],
        },
    }
}

/// An admission policy by counts, never naming a sender or chat.
fn admission_label(admission: &AdmissionPolicy) -> String {
    match admission {
        AdmissionPolicy::All {} => "all senders".to_owned(),
        AdmissionPolicy::Allowlist { sender_ids } => {
            format!("allowlist of {} sender(s)", sender_ids.len())
        }
        AdmissionPolicy::Scoped {
            senders,
            chats,
            groups,
            ..
        } => format!(
            "senders {}; chats {}; groups {}",
            identity_label(senders),
            identity_label(chats),
            group_label(*groups)
        ),
    }
}

fn identity_label(identity: &IdentityAdmission) -> String {
    match identity {
        IdentityAdmission::Deny => "denied".to_owned(),
        IdentityAdmission::Open => "open".to_owned(),
        IdentityAdmission::Allowlist(ids) => format!("allowlist of {}", ids.len()),
    }
}

fn group_label(groups: GroupAdmission) -> &'static str {
    match groups {
        GroupAdmission::Deny => "denied",
        GroupAdmission::All => "all messages",
        GroupAdmission::Mention => "on mention",
        GroupAdmission::ReplyToBot => "on reply to the bot",
        GroupAdmission::MentionOrReplyToBot => "on mention or reply to the bot",
    }
}
