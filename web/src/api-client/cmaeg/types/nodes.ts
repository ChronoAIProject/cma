import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../../schema.js';
import {
  AddressingSchema,
  AgentEventPolicySchema,
  ConsumerIdSchema,
  ConsumerNameSchema,
  ConsumerReplySchema,
  DefaultConsumerSchema,
  DefaultReplySchema,
  PublishPolicySchema,
  ReplyLabelSchema,
  RoutingRuleSchema,
} from './channels.js';

export const SourceCheckStatusSchema = enumeration('passed', 'unavailable');
export type SourceCheckStatus = Output<typeof SourceCheckStatusSchema>;
export const SourceCheckSchema = object({
  id: text,
  label: text,
  status: SourceCheckStatusSchema,
  detail: text,
});
export type SourceCheck = Output<typeof SourceCheckSchema>;
export const SourceCheckResponseSchema = object({
  source_index: integer,
  platform: text,
  scope: text,
  external_delivery_tested: boolean,
  checks: array(SourceCheckSchema),
});
export type SourceCheckResponse = Output<typeof SourceCheckResponseSchema>;
export const ConsumerPolicyRequestSchema = object({
  id: ConsumerIdSchema,
  name: ConsumerNameSchema,
  label: optional(nullable(text)),
  mentionable: optional(boolean),
  non_human: optional(nullable(boolean)),
  reply: optional(ConsumerReplySchema),
  publish: optional(PublishPolicySchema),
});
export type ConsumerPolicyRequest = Output<typeof ConsumerPolicyRequestSchema>;
export const RoutingPolicyRequestSchema = object({
  version: integer,
  default_consumer: DefaultConsumerSchema,
  consumers: optional(array(ConsumerPolicyRequestSchema)),
  rules: optional(array(RoutingRuleSchema)),
  default_targets: array(ConsumerIdSchema),
  addressing: optional(AddressingSchema),
  max_targets: optional(integer),
  reply_label: optional(ReplyLabelSchema),
  default_reply: optional(nullable(DefaultReplySchema)),
  agent_events: optional(AgentEventPolicySchema),
});
export type RoutingPolicyRequest = Output<typeof RoutingPolicyRequestSchema>;
export const RoutingUpdateRequestSchema = object({
  expected_version: integer,
  routing: RoutingPolicyRequestSchema,
});
export type RoutingUpdateRequest = Output<typeof RoutingUpdateRequestSchema>;
