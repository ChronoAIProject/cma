//! Primitives wire types for the independent CMAEG v1 protocol.
use serde::{Deserialize, Serialize};

/// CMAEG `AcceptedStatus` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AcceptedStatus {
    /// Wire value `accepted`.
    #[serde(rename = "accepted")]
    Accepted,
}

/// CMAEG `ActionMode` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionMode {
    /// Wire value `guarded`.
    #[serde(rename = "guarded")]
    Guarded,
    /// Wire value `autonomous`.
    #[serde(rename = "autonomous")]
    Autonomous,
}

/// CMAEG `ActorKind` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActorKind {
    /// Wire value `human`.
    #[serde(rename = "human")]
    Human,
    /// Wire value `bot`.
    #[serde(rename = "bot")]
    Bot,
    /// Wire value `system`.
    #[serde(rename = "system")]
    System,
    /// Wire value `unknown`.
    #[serde(rename = "unknown")]
    Unknown,
}

/// CMAEG `ApiType` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiType {
    /// Wire value `responses`.
    #[serde(rename = "responses")]
    Responses,
    /// Wire value `chat_completions`.
    #[serde(rename = "chat_completions")]
    ChatCompletions,
}

/// CMAEG `AttachmentKind` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachmentKind {
    /// Wire value `image`.
    #[serde(rename = "image")]
    Image,
    /// Wire value `file`.
    #[serde(rename = "file")]
    File,
    /// Wire value `audio`.
    #[serde(rename = "audio")]
    Audio,
    /// Wire value `video`.
    #[serde(rename = "video")]
    Video,
    /// Wire value `other`.
    #[serde(rename = "other")]
    Other,
}

/// CMAEG `BindingId` wire shape.
pub type BindingId = String;

/// CMAEG `ChannelId` wire shape.
pub type ChannelId = String;

/// Opaque delayed-reply authority; do not log or reconstruct it.
pub type ReplyTargetRef = String;

/// CMAEG `ConformanceLevel` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConformanceLevel {
    /// Wire value `l1`.
    #[serde(rename = "l1")]
    L1,
    /// Wire value `l2`.
    #[serde(rename = "l2")]
    L2,
    /// Wire value `l3`.
    #[serde(rename = "l3")]
    L3,
    /// Wire value `l4`.
    #[serde(rename = "l4")]
    L4,
    /// Wire value `l5`.
    #[serde(rename = "l5")]
    L5,
}

/// CMAEG `ConversationId` wire shape.
pub type ConversationId = String;

/// CMAEG `ConversationKind` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversationKind {
    /// Wire value `private`.
    #[serde(rename = "private")]
    Private,
    /// Wire value `group`.
    #[serde(rename = "group")]
    Group,
    /// Wire value `channel`.
    #[serde(rename = "channel")]
    Channel,
    /// Wire value `thread`.
    #[serde(rename = "thread")]
    Thread,
    /// Wire value `custom`.
    #[serde(rename = "custom")]
    Custom,
}

/// CMAEG `EventId` wire shape.
pub type EventId = String;

/// CMAEG `EventRef` wire shape.
pub type EventRef = String;

/// CMAEG `GroupAdmission` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupAdmission {
    /// Wire value `deny`.
    #[serde(rename = "deny")]
    Deny,
    /// Wire value `all`.
    #[serde(rename = "all")]
    All,
    /// Wire value `mention`.
    #[serde(rename = "mention")]
    Mention,
    /// Wire value `reply_to_bot`.
    #[serde(rename = "reply_to_bot")]
    ReplyToBot,
    /// Wire value `mention_or_reply_to_bot`.
    #[serde(rename = "mention_or_reply_to_bot")]
    MentionOrReplyToBot,
}

/// CMAEG `MessageRef` wire shape.
pub type MessageRef = String;

/// CMAEG `ModelAccess` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelAccess {
    /// Wire value `managed`.
    #[serde(rename = "managed")]
    Managed,
    /// Wire value `custom`.
    #[serde(rename = "custom")]
    Custom,
}

/// CMAEG `NyxIdIdentityKind` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NyxIdIdentityKind {
    /// Wire value `human`.
    #[serde(rename = "human")]
    Human,
    /// Wire value `service_account`.
    #[serde(rename = "service_account")]
    ServiceAccount,
    /// Wire value `agent_key`.
    #[serde(rename = "agent_key")]
    AgentKey,
    /// Wire value `organization`.
    #[serde(rename = "organization")]
    Organization,
}

/// CMAEG `OptionStyle` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptionStyle {
    /// Wire value `default`.
    #[serde(rename = "default")]
    Default,
    /// Wire value `primary`.
    #[serde(rename = "primary")]
    Primary,
    /// Wire value `danger`.
    #[serde(rename = "danger")]
    Danger,
}

/// CMAEG `PartitionKey` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartitionKey {
    /// Wire value `conversation_and_sender`.
    #[serde(rename = "conversation_and_sender")]
    ConversationAndSender,
    /// Wire value `conversation`.
    #[serde(rename = "conversation")]
    Conversation,
    /// Wire value `thread`.
    #[serde(rename = "thread")]
    Thread,
    /// Wire value `fixed`.
    #[serde(rename = "fixed")]
    Fixed,
    /// Wire value `event_subject`.
    #[serde(rename = "event_subject")]
    EventSubject,
    /// Wire value `per_event`.
    #[serde(rename = "per_event")]
    PerEvent,
}

/// CMAEG `Platform` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    /// Wire value `telegram`.
    #[serde(rename = "telegram")]
    Telegram,
    /// Wire value `lark`.
    #[serde(rename = "lark")]
    Lark,
    /// Wire value `feishu`.
    #[serde(rename = "feishu")]
    Feishu,
    /// Wire value `discord`.
    #[serde(rename = "discord")]
    Discord,
    /// Wire value `slack`.
    #[serde(rename = "slack")]
    Slack,
    /// Wire value `whatsapp`.
    #[serde(rename = "whatsapp")]
    Whatsapp,
}

/// CMAEG `ProblemCode` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProblemCode {
    /// Wire value `invalid_request`.
    #[serde(rename = "invalid_request")]
    InvalidRequest,
    /// Wire value `not_authenticated`.
    #[serde(rename = "not_authenticated")]
    NotAuthenticated,
    /// Wire value `forbidden`.
    #[serde(rename = "forbidden")]
    Forbidden,
    /// Wire value `channel_not_found`.
    #[serde(rename = "channel_not_found")]
    ChannelNotFound,
    /// Wire value `channel_restore_unavailable`.
    #[serde(rename = "channel_restore_unavailable")]
    ChannelRestoreUnavailable,
    /// Wire value `channel_invalid`.
    #[serde(rename = "channel_invalid")]
    ChannelInvalid,
    /// Wire value `provider_unavailable`.
    #[serde(rename = "provider_unavailable")]
    ProviderUnavailable,
    /// Wire value `provider_rejected`.
    #[serde(rename = "provider_rejected")]
    ProviderRejected,
    /// Wire value `provider_busy`.
    #[serde(rename = "provider_busy")]
    ProviderBusy,
    /// Wire value `draining`.
    #[serde(rename = "draining")]
    Draining,
    /// Wire value `event_ref_invalid`.
    #[serde(rename = "event_ref_invalid")]
    EventRefInvalid,
    /// Wire value `event_ref_expired`.
    #[serde(rename = "event_ref_expired")]
    EventRefExpired,
    /// Wire value `callback_invalid`.
    #[serde(rename = "callback_invalid")]
    CallbackInvalid,
    /// Wire value `admission_refused`.
    #[serde(rename = "admission_refused")]
    AdmissionRefused,
    /// Wire value `rate_limited`.
    #[serde(rename = "rate_limited")]
    RateLimited,
    /// Wire value `conflict`.
    #[serde(rename = "conflict")]
    Conflict,
    /// Wire value `payload_too_large`.
    #[serde(rename = "payload_too_large")]
    PayloadTooLarge,
    /// Wire value `unsupported_media_type`.
    #[serde(rename = "unsupported_media_type")]
    UnsupportedMediaType,
    /// Wire value `internal`.
    #[serde(rename = "internal")]
    Internal,
    /// Wire value `github_signature_invalid`.
    #[serde(rename = "github_signature_invalid")]
    GithubSignatureInvalid,
    /// Wire value `github_installation_forbidden`.
    #[serde(rename = "github_installation_forbidden")]
    GithubInstallationForbidden,
    /// Wire value `github_resource_not_found`.
    #[serde(rename = "github_resource_not_found")]
    GithubResourceNotFound,
    /// Wire value `github_delivery_conflict`.
    #[serde(rename = "github_delivery_conflict")]
    GithubDeliveryConflict,
    /// Wire value `github_source_unavailable`.
    #[serde(rename = "github_source_unavailable")]
    GithubSourceUnavailable,
    /// Wire value `github_dispatch_unavailable`.
    #[serde(rename = "github_dispatch_unavailable")]
    GithubDispatchUnavailable,
    /// Wire value `github_integration_retired`.
    #[serde(rename = "github_integration_retired")]
    GithubIntegrationRetired,
    /// Wire value `github_proof_withdrawn`.
    #[serde(rename = "github_proof_withdrawn")]
    GithubProofWithdrawn,
    /// Wire value `attachment_not_found`.
    #[serde(rename = "attachment_not_found")]
    AttachmentNotFound,
    /// Wire value `media_unavailable`.
    #[serde(rename = "media_unavailable")]
    MediaUnavailable,
}

/// CMAEG `ProviderSlug` wire shape.
pub type ProviderSlug = String;

/// CMAEG `RecordId` wire shape.
pub type RecordId = String;

/// CMAEG `Refusal` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// Wire value `bot_sender`.
    #[serde(rename = "bot_sender")]
    BotSender,
    /// Wire value `system_sender`.
    #[serde(rename = "system_sender")]
    SystemSender,
    /// Wire value `unknown_sender`.
    #[serde(rename = "unknown_sender")]
    UnknownSender,
    /// Wire value `message_edit`.
    #[serde(rename = "message_edit")]
    MessageEdit,
    /// Wire value `message_context_unavailable`.
    #[serde(rename = "message_context_unavailable")]
    MessageContextUnavailable,
    /// Wire value `interactive_action_unsupported`.
    #[serde(rename = "interactive_action_unsupported")]
    InteractiveActionUnsupported,
    /// Wire value `interaction_expired_or_invalid`.
    #[serde(rename = "interaction_expired_or_invalid")]
    InteractionExpiredOrInvalid,
    /// Wire value `system_event`.
    #[serde(rename = "system_event")]
    SystemEvent,
    /// Wire value `content_unsupported`.
    #[serde(rename = "content_unsupported")]
    ContentUnsupported,
    /// Wire value `attachment_unsupported`.
    #[serde(rename = "attachment_unsupported")]
    AttachmentUnsupported,
    /// Wire value `sender_denied`.
    #[serde(rename = "sender_denied")]
    SenderDenied,
    /// Wire value `chat_denied`.
    #[serde(rename = "chat_denied")]
    ChatDenied,
    /// Wire value `group_denied`.
    #[serde(rename = "group_denied")]
    GroupDenied,
    /// Wire value `group_not_addressed`.
    #[serde(rename = "group_not_addressed")]
    GroupNotAddressed,
    /// Wire value `addressing_evidence_unavailable`.
    #[serde(rename = "addressing_evidence_unavailable")]
    AddressingEvidenceUnavailable,
    /// Wire value `thread_evidence_unavailable`.
    #[serde(rename = "thread_evidence_unavailable")]
    ThreadEvidenceUnavailable,
    /// Wire value `platform_verifier_unavailable`.
    #[serde(rename = "platform_verifier_unavailable")]
    PlatformVerifierUnavailable,
}

/// CMAEG `RenderingOutcome` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderingOutcome {
    /// Wire value `rendered`.
    #[serde(rename = "rendered")]
    Rendered,
    /// Wire value `degraded`.
    #[serde(rename = "degraded")]
    Degraded,
    /// Wire value `unsupported`.
    #[serde(rename = "unsupported")]
    Unsupported,
}

/// CMAEG `Secret` wire shape.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

/// CMAEG `SkillSource` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    /// Wire value `ornn`.
    #[serde(rename = "ornn")]
    Ornn,
}

/// CMAEG `ThinkingEffort` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThinkingEffort {
    /// Wire value `minimal`.
    #[serde(rename = "minimal")]
    Minimal,
    /// Wire value `low`.
    #[serde(rename = "low")]
    Low,
    /// Wire value `medium`.
    #[serde(rename = "medium")]
    Medium,
    /// Wire value `high`.
    #[serde(rename = "high")]
    High,
    /// Wire value `very_high`.
    #[serde(rename = "very_high")]
    VeryHigh,
}

/// CMAEG `TypingOutcome` wire shape.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypingOutcome {
    /// Wire value `sent`.
    #[serde(rename = "sent")]
    Sent,
    /// Wire value `unsupported`.
    #[serde(rename = "unsupported")]
    Unsupported,
}
