import { enumeration, text, type Output } from '../../schema.js';

export const AcceptedStatusSchema = enumeration('accepted');
export type AcceptedStatus = Output<typeof AcceptedStatusSchema>;

export const ActionModeSchema = enumeration('guarded', 'autonomous');
export type ActionMode = Output<typeof ActionModeSchema>;

export const ActorKindSchema = enumeration('human', 'bot', 'system', 'unknown');
export type ActorKind = Output<typeof ActorKindSchema>;

export const ConversationKindSchema = enumeration(
  'private',
  'group',
  'channel',
  'thread',
  'custom',
);
export type ConversationKind = Output<typeof ConversationKindSchema>;

export const EventIdSchema = text;
export type EventId = Output<typeof EventIdSchema>;

export const AttachmentKindSchema = enumeration('image', 'file', 'audio', 'video', 'other');
export type AttachmentKind = Output<typeof AttachmentKindSchema>;

export const MediaVariantSchema = enumeration(
  'photo',
  'document',
  'audio',
  'voice',
  'video',
  'video_note',
  'animation',
  'sticker',
  'image',
  'file',
  'media',
  'embedded',
);
export type MediaVariant = Output<typeof MediaVariantSchema>;

export const MessageRefSchema = text;
export type MessageRef = Output<typeof MessageRefSchema>;
export const ReplyTargetRefSchema = text;
export type ReplyTargetRef = Output<typeof ReplyTargetRefSchema>;

export const PlatformSchema = enumeration(
  'telegram',
  'lark',
  'feishu',
  'discord',
  'slack',
  'whatsapp',
);
export type Platform = Output<typeof PlatformSchema>;

export const GroupAdmissionSchema = enumeration(
  'deny',
  'all',
  'mention',
  'reply_to_bot',
  'mention_or_reply_to_bot',
);
export type GroupAdmission = Output<typeof GroupAdmissionSchema>;

export const SkillSourceSchema = enumeration('ornn');
export type SkillSource = Output<typeof SkillSourceSchema>;

export const NyxIdIdentityKindSchema = enumeration(
  'human',
  'service_account',
  'agent_key',
  'organization',
);
export type NyxIdIdentityKind = Output<typeof NyxIdIdentityKindSchema>;

export const ProviderSlugSchema = text;
export type ProviderSlug = Output<typeof ProviderSlugSchema>;

export const ModelAccessSchema = enumeration('managed', 'custom');
export type ModelAccess = Output<typeof ModelAccessSchema>;

export const ApiTypeSchema = enumeration('responses', 'chat_completions');
export type ApiType = Output<typeof ApiTypeSchema>;

export const ThinkingEffortSchema = enumeration('minimal', 'low', 'medium', 'high', 'very_high');
export type ThinkingEffort = Output<typeof ThinkingEffortSchema>;

export const BindingIdSchema = text;
export type BindingId = Output<typeof BindingIdSchema>;

export const OptionStyleSchema = enumeration('default', 'primary', 'danger');
export type OptionStyle = Output<typeof OptionStyleSchema>;

export const SecretSchema = text;
export type Secret = Output<typeof SecretSchema>;

export const RefusalSchema = enumeration(
  'bot_sender',
  'system_sender',
  'unknown_sender',
  'message_edit',
  'interactive_action_unsupported',
  'interaction_expired_or_invalid',
  'system_event',
  'content_unsupported',
  'attachment_unsupported',
  'sender_denied',
  'chat_denied',
  'group_denied',
  'group_not_addressed',
  'addressing_evidence_unavailable',
  'thread_evidence_unavailable',
  'platform_verifier_unavailable',
);
export type Refusal = Output<typeof RefusalSchema>;

export const PartitionKeySchema = enumeration(
  'conversation_and_sender',
  'conversation',
  'thread',
  'fixed',
  'event_subject',
  'per_event',
);
export type PartitionKey = Output<typeof PartitionKeySchema>;

export const RecordIdSchema = text;
export type RecordId = Output<typeof RecordIdSchema>;

export const ChannelIdSchema = text;
export type ChannelId = Output<typeof ChannelIdSchema>;

export const ConformanceLevelSchema = enumeration('l1', 'l2', 'l3', 'l4', 'l5');
export type ConformanceLevel = Output<typeof ConformanceLevelSchema>;

export const ConversationIdSchema = text;
export type ConversationId = Output<typeof ConversationIdSchema>;

export const EventRefSchema = text;
export type EventRef = Output<typeof EventRefSchema>;

export const RenderingOutcomeSchema = enumeration('rendered', 'degraded', 'unsupported');
export type RenderingOutcome = Output<typeof RenderingOutcomeSchema>;

export const ProblemCodeSchema = enumeration(
  'invalid_request',
  'not_authenticated',
  'forbidden',
  'channel_not_found',
  'channel_restore_unavailable',
  'channel_invalid',
  'provider_unavailable',
  'provider_rejected',
  'provider_busy',
  'draining',
  'event_ref_invalid',
  'event_ref_expired',
  'callback_invalid',
  'admission_refused',
  'rate_limited',
  'conflict',
  'payload_too_large',
  'unsupported_media_type',
  'internal',
  'attachment_not_found',
  'media_unavailable',
);
export type ProblemCode = Output<typeof ProblemCodeSchema>;

export const TypingOutcomeSchema = enumeration('sent', 'unsupported');
export type TypingOutcome = Output<typeof TypingOutcomeSchema>;
