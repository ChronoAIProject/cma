use layer2_core::{bots::*, profiles::AgentProfileRef, AgentProfileId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateBotBody {
    pub name: String,
    pub nyxid_bot_id: String,
    pub profile: ProfileRef,
    pub admission: Admission,
    pub reply: Reply,
}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileRef {
    pub id: String,
    pub revision: u32,
}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Admission {
    All {},
    Allowlist {
        sender_ids: Vec<String>,
    },
    Scoped {
        senders: IdentityPolicy,
        chats: IdentityPolicy,
        groups: GroupPolicy,
    },
}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum IdentityPolicy {
    Deny {},
    Open {},
    Allowlist { ids: Vec<String> },
}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    Deny,
    All,
    Mention,
    ReplyToBot,
    MentionOrReplyToBot,
}
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub auto_final: bool,
    pub ack_after_ms: u32,
}
impl CreateBotBody {
    pub fn into_core(self) -> Result<CreateBot, BotError> {
        Ok(CreateBot {
            name: self.name,
            external_bot_id: self.nyxid_bot_id,
            profile: AgentProfileRef {
                id: AgentProfileId(self.profile.id),
                revision: self.profile.revision,
            },
            admission: serde_json::from_value(
                serde_json::to_value(self.admission).map_err(|_| BotError::Unavailable)?,
            )
            .map_err(|_| BotError::Invalid("invalid admission policy".into()))?,
            reply: BotReply {
                auto_final: self.reply.auto_final,
                ack_after_ms: self.reply.ack_after_ms,
            },
        })
    }
}
#[derive(Serialize, ToSchema)]
pub struct BotView {
    pub bot_id: String,
    pub owner: String,
    pub name: String,
    pub nyxid_bot_id: String,
    pub profile: ProfileRef,
    pub provider: String,
    pub state: String,
    pub failure: Option<String>,
    pub operation: Option<OperationView>,
    pub channel: Option<ChannelView>,
    pub credentials_expire_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub last_test: Option<TestView>,
}
#[derive(Serialize, ToSchema)]
pub struct OperationView {
    pub operation: String,
    pub step: String,
}
#[derive(Serialize, ToSchema)]
pub struct ChannelView {
    pub channel_id: String,
    pub record_id: String,
    pub version: u64,
    pub binding_id: String,
    pub level: String,
    pub callback_url: String,
    pub event_url: String,
}
#[derive(Serialize, ToSchema)]
pub struct TestView {
    pub accepted_after_ms: u64,
    pub first_text: String,
    pub level: String,
    pub tested_at: String,
}
#[derive(Serialize, ToSchema)]
pub struct BotPage {
    pub items: Vec<BotView>,
    pub next_after: Option<String>,
}
#[derive(Serialize, ToSchema)]
pub struct ExternalBotView {
    pub id: String,
    pub name: String,
    pub platform: String,
}
impl From<BotTestResult> for TestView {
    fn from(r: BotTestResult) -> Self {
        Self {
            accepted_after_ms: r.accepted_after_ms,
            first_text: r.first_text,
            level: r.level,
            tested_at: r.tested_at,
        }
    }
}
impl From<Bot> for BotView {
    fn from(bot: Bot) -> Self {
        let (state, failure) = match bot.state {
            BotState::Creating => ("creating", None),
            BotState::Active => ("active", None),
            BotState::Disabled => ("disabled", None),
            BotState::Failed { reason } => ("failed", Some(reason)),
            BotState::Deleting => ("deleting", None),
            BotState::Deleted => ("deleted", None),
        };
        Self {
            bot_id: bot.bot_id,
            owner: bot.owner.to_string(),
            name: bot.request.name,
            nyxid_bot_id: bot.request.external_bot_id,
            profile: ProfileRef {
                id: bot.request.profile.id.0,
                revision: bot.request.profile.revision,
            },
            provider: bot.provider,
            state: state.into(),
            failure,
            operation: bot.work.map(|w| OperationView {
                operation: match w.operation {
                    BotOperation::Create => "create",
                    BotOperation::Disable => "disable",
                    BotOperation::Enable => "enable",
                    BotOperation::Rotate => "rotate",
                    BotOperation::Delete => "delete",
                    BotOperation::Cleanup => "cleanup",
                }
                .into(),
                step: w.step,
            }),
            channel: bot.resources.channel.map(|c| ChannelView {
                channel_id: c.channel_id,
                record_id: c.record_id,
                version: c.version,
                binding_id: c.binding_id,
                level: c.level,
                callback_url: c.callback_url,
                event_url: c.event_url,
            }),
            credentials_expire_at: bot.resources.credentials.expires_at,
            created_at: bot.created_at,
            updated_at: bot.updated_at,
            last_test: bot.last_test.map(Into::into),
        }
    }
}
