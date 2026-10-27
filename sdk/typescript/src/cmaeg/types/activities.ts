import { ConversationHistoryCapabilitiesSchema } from './conversation-history.js';
import { ThreadCapabilitiesSchema } from './thread-lifecycle.js';
import { LarkRegionSchema } from './primitives.js';
import { LarkResourceBindingSchema } from './channels.js';
import { NativeMediaCapabilitiesSchema, NativeMediaEditCapabilitiesSchema } from './messages.js';
import { MessageEffectsCapabilitiesSchema } from './message-effects.js';
import {
  AcceptedStatusSchema,
  ActorKindSchema,
  AttachmentKindSchema,
  BindingIdSchema,
  ChannelIdSchema,
  ConversationIdSchema,
  ConversationKindSchema,
  EventIdSchema,
  EventRefSchema,
  MediaVariantSchema,
  MessageRefSchema,
  ReplyTargetRefSchema,
  PlatformSchema,
  ProblemCodeSchema,
} from './primitives.js';
import {
  array,
  boolean,
  dictionary,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  tagged,
  text,
  type Output,
} from '../../schema.js';
import { anything, extensible } from './helpers.js';
import { InboundMediaCapabilitiesSchema } from './media.js';
import {
  ConsumerIdSchema,
  ConsumerNameSchema,
  ReplyChoiceSchema,
  RuleIdSchema,
  SourceIdSchema,
} from './channels.js';

export const ReplyTargetResponseSchema = object({
  reply_target_ref: ReplyTargetRefSchema,
  expires_at: integer,
});
export type ReplyTargetResponse = Output<typeof ReplyTargetResponseSchema>;

export const ActorSchema = object({
  display_name: optional(nullable(text)),
  id: text,
  kind: ActorKindSchema,
});
export type Actor = Output<typeof ActorSchema>;

export const ActivityConversationSchema = object({
  id: text,
  kind: ConversationKindSchema,
  thread_id: optional(nullable(text)),
});
export type ActivityConversation = Output<typeof ActivityConversationSchema>;

export const LarkResourceKindSchema = enumeration('image', 'file');
export type LarkResourceKind = Output<typeof LarkResourceKindSchema>;

// A non-secret locator bound into the activity digest; fetching still requires
// the event reference's authority.
export const AttachmentSourceSchema = tagged(
  {
    whatsapp: object({
      type: enumeration('whatsapp'),
      message_id: text,
      media_id: text,
      sha256: text,
    }),
    discord: object({
      channel_id: text,
      message_id: text,
      attachment_id: text,
      type: enumeration('discord'),
    }),
    nyxid_relay: object({ index: integer, type: enumeration('nyxid_relay') }),
    native_lark: object({
      type: enumeration('native_lark'),
      message_id: text,
      resource_key: text,
      resource_kind: LarkResourceKindSchema,
      source_revision: text,
    }),
    lark_resource: object({
      type: enumeration('lark_resource'),
      message_id: text,
      resource_key: text,
      resource_kind: LarkResourceKindSchema,
      binding: LarkResourceBindingSchema,
      bot_id: text,
      source_revision: text,
    }),
    telegram: object({
      file_id: text,
      file_unique_id: text,
      type: enumeration('telegram'),
    }),
    url: object({ type: enumeration('url'), url: text }),
    inline: object({ pointer: text, type: enumeration('inline') }),
  },
  'type',
);
export type AttachmentSource = Output<typeof AttachmentSourceSchema>;

export const AttachmentSchema = object({
  duration_seconds: optional(nullable(integer)),
  emoji: optional(nullable(text)),
  filename: optional(nullable(text)),
  height: optional(nullable(integer)),
  kind: AttachmentKindSchema,
  mime_type: optional(nullable(text)),
  reference: text,
  size_bytes: optional(nullable(integer)),
  source: optional(nullable(AttachmentSourceSchema)),
  thumbnail: optional(nullable(AttachmentSourceSchema)),
  variant: optional(nullable(MediaVariantSchema)),
  width: optional(nullable(integer)),
});
export type Attachment = Output<typeof AttachmentSchema>;

export const InteractionResultSchema = object({
  action_id: text,
  block_id: text,
  values: optional(dictionary(anything)),
});
export type InteractionResult = Output<typeof InteractionResultSchema>;

export const CloudEventSchema = extensible({
  data: optional(anything),
  datacontenttype: optional(nullable(text)),
  dataschema: optional(nullable(text)),
  id: text,
  source: text,
  specversion: text,
  subject: optional(nullable(text)),
  time: optional(nullable(text)),
  type: text,
});
export type CloudEvent = Output<typeof CloudEventSchema>;

export const ActivityKindSchema = tagged(
  {
    message: object({
      album_id: optional(nullable(text)),
      attachments: optional(array(AttachmentSchema)),
      mentions_bot: boolean,
      reply_to: optional(nullable(MessageRefSchema)),
      text: text,
      type: enumeration('message'),
    }),
    interaction: object({
      action_id: text,
      block_id: text,
      values: optional(dictionary(anything)),
      type: enumeration('interaction'),
    }),
    command: object({ args: text, name: text, type: enumeration('command') }),
    event: object({
      attachments: optional(array(AttachmentSchema)),
      ce: CloudEventSchema,
      type: enumeration('event'),
    }),
  },
  'type',
);
export type ActivityKind = Output<typeof ActivityKindSchema>;

export const SourceKindSchema = tagged(
  {
    native_lark: object({
      type: enumeration('native_lark'),
      region: LarkRegionSchema,
      app_id: text,
      tenant_key: text,
      bot_open_id: text,
    }),
    whatsapp: object({
      app_id: text,
      waba_id: text,
      phone_number_id: text,
      type: enumeration('whatsapp'),
    }),
    discord: object({ bot_id: text, type: enumeration('discord') }),
    telegram: object({
      platform: PlatformSchema,
      bot_id: text,
      type: enumeration('telegram'),
    }),
    nyxid_relay: object({
      platform: PlatformSchema,
      route_id: text,
      type: enumeration('nyxid_relay'),
    }),
    custom: object({ type: enumeration('custom') }),
  },
  'type',
);
export type SourceKind = Output<typeof SourceKindSchema>;

export const ThreadKindSchema = enumeration('native');
export type ThreadKind = Output<typeof ThreadKindSchema>;
export const ThreadAddressSchema = enumeration(
  'mention',
  'reply_to_bot',
  'not_addressed',
  'unknown',
);
export type ThreadAddress = Output<typeof ThreadAddressSchema>;
export const ThreadFactsSchema = object({
  version: integer,
  kind: ThreadKindSchema,
  chat_id: text,
  message_id: text,
  root_id: optional(nullable(text)),
  native_thread_id: optional(nullable(text)),
  parent_message_id: optional(nullable(text)),
  sender_kind: ActorKindSchema,
  address: ThreadAddressSchema,
  mentions_others: optional(boolean),
});
export type ThreadFacts = Output<typeof ThreadFactsSchema>;

export const RouteResolutionSchema = enumeration(
  'default',
  'rule',
  'mention',
  'command',
  'event_field',
  'interaction',
  'agent_event',
);
export type RouteResolution = Output<typeof RouteResolutionSchema>;

export const ConsumerTargetSchema = object({ id: ConsumerIdSchema, name: ConsumerNameSchema });
export type ConsumerTarget = Output<typeof ConsumerTargetSchema>;

// Which of a routed channel's consumers a context response was read for.
export const ConsumerSummarySchema = object({
  id: ConsumerIdSchema,
  label: text,
  name: ConsumerNameSchema,
});
export type ConsumerSummary = Output<typeof ConsumerSummarySchema>;

// What caused a hand-off: the event it continues and its record-scoped chain.
export const CausationSchema = object({
  hop: integer,
  origin_source_id: SourceIdSchema,
  parent_event_id: EventIdSchema,
  published_by: ConsumerIdSchema,
  reply: ReplyChoiceSchema,
  root: text,
});
export type Causation = Output<typeof CausationSchema>;

// Which consumers a routed event reaches; targets[0] is the primary consumer.
export const ActivityRoutingSchema = object({
  addressing_bytes: optional(integer),
  causation: optional(nullable(CausationSchema)),
  resolution: RouteResolutionSchema,
  rule_id: optional(nullable(RuleIdSchema)),
  source_id: SourceIdSchema,
  targets: array(ConsumerTargetSchema),
});
export type ActivityRouting = Output<typeof ActivityRoutingSchema>;

export const ActivitySchema = object({
  thread: optional(nullable(ThreadFactsSchema)),
  actor: ActorSchema,
  conversation: ActivityConversationSchema,
  event_id: EventIdSchema,
  kind: ActivityKindSchema,
  occurred_at: text,
  routing: optional(nullable(ActivityRoutingSchema)),
  schema_version: integer,
  source: SourceKindSchema,
});
export type Activity = Output<typeof ActivitySchema>;

export const ThreadHistoryMessageSchema = object({
  message_id: text,
  author: ActorSchema,
  text,
  occurred_at: text,
  edited_at: optional(nullable(text)),
  attachment_count: integer,
});
export type ThreadHistoryMessage = Output<typeof ThreadHistoryMessageSchema>;
export const ThreadHistorySchema = object({
  thread_id: text,
  before_message_id: text,
  messages: array(ThreadHistoryMessageSchema),
  text_truncated: boolean,
});
export type ThreadHistory = Output<typeof ThreadHistorySchema>;

export const CapabilitiesSchema = object({
  native_media_edits: optional(nullable(NativeMediaEditCapabilitiesSchema)),
  thread_history: optional(nullable(boolean)),
  thread_lifecycle: optional(nullable(ThreadCapabilitiesSchema)),
  conversation_history: optional(nullable(ConversationHistoryCapabilitiesSchema)),
  message_effects: optional(nullable(MessageEffectsCapabilitiesSchema)),
  whatsapp_templates: optional(nullable(boolean)),
  whatsapp_template_components: optional(nullable(array(text))),
  native_media: optional(nullable(NativeMediaCapabilitiesSchema)),
  attachments: text,
  forms: optional(nullable(boolean)),
  buttons: text,
  cards: text,
  edit: boolean,
  inbound_media: optional(nullable(InboundMediaCapabilitiesSchema)),
  max_text_chars: integer,
  typing: boolean,
});
export type Capabilities = Output<typeof CapabilitiesSchema>;

export const PublicActivitySchema = object({
  actor: ActorSchema,
  conversation: ActivityConversationSchema,
  kind: ActivityKindSchema,
  occurred_at: text,
  routing: optional(nullable(ActivityRoutingSchema)),
});
export type PublicActivity = Output<typeof PublicActivitySchema>;

export const ReceivedMessageContextSchema = object({
  message_ref: text,
  effects: MessageEffectsCapabilitiesSchema,
});
export type ReceivedMessageContext = Output<typeof ReceivedMessageContextSchema>;

export const ContextResponseSchema = object({
  received_message: optional(nullable(ReceivedMessageContextSchema)),
  thread_history: optional(nullable(ThreadHistorySchema)),
  activity: PublicActivitySchema,
  activity_verified: boolean,
  capabilities: CapabilitiesSchema,
  channel_id: ChannelIdSchema,
  consumer: optional(nullable(ConsumerSummarySchema)),
  conversation_id: ConversationIdSchema,
  event_id: EventIdSchema,
  expires_at: integer,
  provider_binding_id: BindingIdSchema,
  source: SourceKindSchema,
});
export type ContextResponse = Output<typeof ContextResponseSchema>;

export const EventAcceptedSchema = object({
  conversation_id: ConversationIdSchema,
  event_id: EventIdSchema,
  status: AcceptedStatusSchema,
});
export type EventAccepted = Output<typeof EventAcceptedSchema>;

export const EventContextSnapshotSchema = object({
  activity: ActivitySchema,
  channel_id: ChannelIdSchema,
  conversation_id: ConversationIdSchema,
  event_ref: EventRefSchema,
  schema_version: integer,
});
export type EventContextSnapshot = Output<typeof EventContextSnapshotSchema>;

export const ProblemSchema = object({
  code: ProblemCodeSchema,
  detail: optional(nullable(text)),
  instance: optional(nullable(text)),
  status: integer,
  title: text,
  type: optional(text),
});
export type Problem = Output<typeof ProblemSchema>;
