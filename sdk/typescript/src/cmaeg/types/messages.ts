import { WhatsappTemplateSchema } from './whatsapp-template.js';
import {
  AttachmentKindSchema,
  MessageRefSchema,
  OptionStyleSchema,
  RenderingOutcomeSchema,
  TypingOutcomeSchema,
} from './primitives.js';
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

export const FieldSchema = object({ label: text, value: text });
export type Field = Output<typeof FieldSchema>;

export const ChoiceOptionSchema = object({
  id: text,
  label: text,
  selected: optional(boolean),
  style: optional(OptionStyleSchema),
});
export type ChoiceOption = Output<typeof ChoiceOptionSchema>;

export const FormFieldKindSchema = tagged(
  {
    text: object({ type: enumeration('text') }),
    number: object({ type: enumeration('number') }),
    choice: object({ options: array(ChoiceOptionSchema), type: enumeration('choice') }),
    date: object({ type: enumeration('date') }),
  },
  'type',
);
export type FormFieldKind = Output<typeof FormFieldKindSchema>;

export const FormFieldSchema = object({
  id: text,
  kind: FormFieldKindSchema,
  label: text,
  required: boolean,
});
export type FormField = Output<typeof FormFieldSchema>;

export const BlockSchema = tagged(
  {
    text: object({ text: text, type: enumeration('text') }),
    card: object({
      body: optional(nullable(text)),
      fields: optional(array(FieldSchema)),
      image: optional(nullable(text)),
      title: text,
      type: enumeration('card'),
    }),
    buttons: object({ id: text, options: array(ChoiceOptionSchema), type: enumeration('buttons') }),
    choice: object({
      id: text,
      multi: boolean,
      options: array(ChoiceOptionSchema),
      prompt: text,
      type: enumeration('choice'),
    }),
    form: object({ fields: array(FormFieldSchema), id: text, type: enumeration('form') }),
    attachment: object({
      caption: optional(nullable(text)),
      kind: AttachmentKindSchema,
      reference: text,
      type: enumeration('attachment'),
    }),
    link: object({ label: optional(nullable(text)), type: enumeration('link'), url: text }),
  },
  'type',
);
export type Block = Output<typeof BlockSchema>;

export const BlockRenderingOutcomeSchema = tagged(
  {
    exact: object({ type: enumeration('exact') }),
    degraded: object({ how: text, type: enumeration('degraded') }),
    dropped: object({ type: enumeration('dropped'), why: text }),
  },
  'type',
);
export type BlockRenderingOutcome = Output<typeof BlockRenderingOutcomeSchema>;

export const BlockRenderingSchema = object({
  index: integer,
  outcome: BlockRenderingOutcomeSchema,
});
export type BlockRendering = Output<typeof BlockRenderingSchema>;

export const DeliveryOutcomeSchema = tagged(
  {
    confirmed: object({
      outcome: enumeration('confirmed'),
      platform_message_id: optional(nullable(text)),
    }),
    rejected: object({ outcome: enumeration('rejected'), reason: text }),
    rate_limited: object({ outcome: enumeration('rate_limited'), retry_after: integer }),
    unknown: object({ outcome: enumeration('unknown'), reason: text }),
    unsupported: object({ outcome: enumeration('unsupported') }),
  },
  'outcome',
);
export type DeliveryOutcome = Output<typeof DeliveryOutcomeSchema>;

export const RenderingReportSchema = object({
  blocks: optional(array(BlockRenderingSchema)),
  outcome: RenderingOutcomeSchema,
});
export type RenderingReport = Output<typeof RenderingReportSchema>;

export const IntentReceiptSchema = object({
  delivery: DeliveryOutcomeSchema,
  message_ref: optional(nullable(MessageRefSchema)),
  rendering: RenderingReportSchema,
});
export type IntentReceipt = Output<typeof IntentReceiptSchema>;

export const NativeMediaKindSchema = enumeration('image', 'file', 'audio', 'video', 'sticker');
export type NativeMediaKind = Output<typeof NativeMediaKindSchema>;
export const NativeMediaSourceSchema = tagged(
  {
    url: object({ type: enumeration('url'), url: text }),
    base64: object({ type: enumeration('base64'), data: text }),
  },
  'type',
);
export type NativeMediaSource = Output<typeof NativeMediaSourceSchema>;
export const NativeAttachmentSchema = object({
  kind: NativeMediaKindSchema,
  source: NativeMediaSourceSchema,
  filename: optional(nullable(text)),
  mime_type: optional(nullable(text)),
  caption: optional(nullable(text)),
});
export type NativeAttachment = Output<typeof NativeAttachmentSchema>;
export const NativeStickerCapabilitiesSchema = object({
  width: integer,
  height: integer,
  max_static_bytes: integer,
  max_animated_bytes: integer,
  max_frames: integer,
  max_duration_ms: integer,
  min_frame_duration_ms: integer,
});
export type NativeStickerCapabilities = Output<typeof NativeStickerCapabilitiesSchema>;
export const NativeVideoCapabilitiesSchema = object({
  video_codec: text,
  video_profile: text,
  audio_codec: text,
  max_audio_tracks: integer,
  max_width: integer,
  max_height: integer,
  max_frame_rate: integer,
  max_duration_ms: integer,
  max_samples: integer,
  progressive_only: boolean,
  max_slices_per_frame: integer,
  fast_start_required: boolean,
});
export type NativeVideoCapabilities = Output<typeof NativeVideoCapabilitiesSchema>;
export const NativeMediaCapabilitiesSchema = object({
  kinds: array(text),
  sources: array(text),
  max_attachments: integer,
  max_inline_bytes: integer,
  mime_types: optional(nullable(array(text))),
  max_total_bytes: optional(nullable(integer)),
  max_image_bytes: optional(nullable(integer)),
  max_caption_chars: optional(nullable(integer)),
  sticker: optional(nullable(NativeStickerCapabilitiesSchema)),
  video: optional(nullable(NativeVideoCapabilitiesSchema)),
});
export type NativeMediaCapabilities = Output<typeof NativeMediaCapabilitiesSchema>;

/** Explicit update patch: omission preserves and an empty string clears. Null is invalid. */
export const NativeMediaReplacementSchema = object({
  kind: NativeMediaKindSchema,
  source: NativeMediaSourceSchema,
  filename: optional(nullable(text)),
  mime_type: optional(nullable(text)),
});
export type NativeMediaReplacement = Output<typeof NativeMediaReplacementSchema>;
export const NativeMediaUpdateSchema = object({
  replacement: optional(NativeMediaReplacementSchema),
  caption: optional(text),
  text: optional(text),
});
export type NativeMediaUpdate = Output<typeof NativeMediaUpdateSchema>;
export const NativeMediaEditCapabilitiesSchema = object({
  caption: boolean,
  content: boolean,
  replacement_kinds: array(NativeMediaKindSchema),
  max_attachments: integer,
  max_bytes: integer,
  idempotency_key_required: boolean,
  expires_at: optional(nullable(integer)),
});
export type NativeMediaEditCapabilities = Output<typeof NativeMediaEditCapabilitiesSchema>;

export const OutboundMessageSchema = object({
  native_media_update: optional(NativeMediaUpdateSchema),
  whatsapp_template: optional(nullable(WhatsappTemplateSchema)),
  native_attachments: optional(array(NativeAttachmentSchema)),
  thread_reply: optional(boolean),
  blocks: optional(array(BlockSchema)),
  reply_to: optional(nullable(MessageRefSchema)),
  text: optional(nullable(text)),
});
export type OutboundMessage = Output<typeof OutboundMessageSchema>;

export const TypingReceiptSchema = object({ outcome: TypingOutcomeSchema });
export type TypingReceipt = Output<typeof TypingReceiptSchema>;
