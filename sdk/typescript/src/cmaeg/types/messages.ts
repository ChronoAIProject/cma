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

export const OutboundMessageSchema = object({
  blocks: optional(array(BlockSchema)),
  reply_to: optional(nullable(MessageRefSchema)),
  text: optional(nullable(text)),
});
export type OutboundMessage = Output<typeof OutboundMessageSchema>;

export const TypingReceiptSchema = object({ outcome: TypingOutcomeSchema });
export type TypingReceipt = Output<typeof TypingReceiptSchema>;
