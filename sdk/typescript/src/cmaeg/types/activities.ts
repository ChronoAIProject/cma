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

export const AttachmentSchema = object({
  filename: optional(nullable(text)),
  kind: AttachmentKindSchema,
  mime_type: optional(nullable(text)),
  reference: text,
  size_bytes: optional(nullable(integer)),
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
    event: object({ ce: CloudEventSchema, type: enumeration('event') }),
  },
  'type',
);
export type ActivityKind = Output<typeof ActivityKindSchema>;

export const SourceKindSchema = tagged(
  {
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

export const ActivitySchema = object({
  actor: ActorSchema,
  conversation: ActivityConversationSchema,
  event_id: EventIdSchema,
  kind: ActivityKindSchema,
  occurred_at: text,
  schema_version: integer,
  source: SourceKindSchema,
});
export type Activity = Output<typeof ActivitySchema>;

export const CapabilitiesSchema = object({
  attachments: text,
  buttons: text,
  cards: text,
  edit: boolean,
  max_text_chars: integer,
  typing: boolean,
});
export type Capabilities = Output<typeof CapabilitiesSchema>;

export const PublicActivitySchema = object({
  actor: ActorSchema,
  conversation: ActivityConversationSchema,
  kind: ActivityKindSchema,
  occurred_at: text,
});
export type PublicActivity = Output<typeof PublicActivitySchema>;

export const ContextResponseSchema = object({
  activity: PublicActivitySchema,
  activity_verified: boolean,
  capabilities: CapabilitiesSchema,
  channel_id: ChannelIdSchema,
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
