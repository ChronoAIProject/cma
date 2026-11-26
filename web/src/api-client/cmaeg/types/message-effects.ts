import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  tagged,
  text,
  type Output,
} from '../../schema.js';
import { DeliveryOutcomeSchema, NativeMediaEditCapabilitiesSchema } from './messages.js';

/** Retained wire shape; Lark/Feishu use emoji_type names such as THUMBSUP in emoji. */
export const TelegramReactionSchema = tagged(
  {
    emoji: object({ type: enumeration('emoji'), emoji: text }),
    custom_emoji: object({ type: enumeration('custom_emoji'), custom_emoji_id: text }),
  },
  'type',
);
export type TelegramReaction = Output<typeof TelegramReactionSchema>;
export const MessageReactionRequestSchema = object({
  reaction: array(TelegramReactionSchema),
  is_big: optional(boolean),
});
export type MessageReactionRequest = Output<typeof MessageReactionRequestSchema>;
export const MessageEffectsCapabilitiesSchema = object({
  media_edit: optional(nullable(NativeMediaEditCapabilitiesSchema)),
  delete: boolean,
  reaction: boolean,
  scope: text,
  max_reactions: integer,
  expires_at: optional(nullable(integer)),
});
export type MessageEffectsCapabilities = Output<typeof MessageEffectsCapabilitiesSchema>;
export const MessageEffectReceiptSchema = object({ delivery: DeliveryOutcomeSchema });
export type MessageEffectReceipt = Output<typeof MessageEffectReceiptSchema>;
export const EmptyEffectRequestSchema = object({});
export type EmptyEffectRequest = Output<typeof EmptyEffectRequestSchema>;

export const SideMessageBodySchema = object({ text });
export type SideMessageBody = Output<typeof SideMessageBodySchema>;
export const SideMessageCapabilitySchema = object({ available: boolean });
export type SideMessageCapability = Output<typeof SideMessageCapabilitySchema>;
