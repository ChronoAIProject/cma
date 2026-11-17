import {
  array,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  tagged,
  text,
  type Output,
} from '../../schema.js';

export const WhatsappTemplateMediaSchema = object({
  id: optional(nullable(text)),
  link: optional(nullable(text)),
});
export type WhatsappTemplateMedia = Output<typeof WhatsappTemplateMediaSchema>;
export const WhatsappTemplateDocumentSchema = object({
  id: optional(nullable(text)),
  link: optional(nullable(text)),
  filename: optional(nullable(text)),
});
export type WhatsappTemplateDocument = Output<typeof WhatsappTemplateDocumentSchema>;
export const WhatsappTemplateCurrencySchema = object({
  fallback_value: text,
  code: text,
  amount_1000: integer,
});
export type WhatsappTemplateCurrency = Output<typeof WhatsappTemplateCurrencySchema>;
export const WhatsappTemplateDateTimeSchema = object({ fallback_value: text });
export type WhatsappTemplateDateTime = Output<typeof WhatsappTemplateDateTimeSchema>;

export const WhatsappTemplateParameterSchema = tagged(
  {
    text: object({ type: enumeration('text'), text }),
    currency: object({ type: enumeration('currency'), currency: WhatsappTemplateCurrencySchema }),
    date_time: object({
      type: enumeration('date_time'),
      date_time: WhatsappTemplateDateTimeSchema,
    }),
    image: object({ type: enumeration('image'), image: WhatsappTemplateMediaSchema }),
    video: object({ type: enumeration('video'), video: WhatsappTemplateMediaSchema }),
    document: object({ type: enumeration('document'), document: WhatsappTemplateDocumentSchema }),
  },
  'type',
);
export type WhatsappTemplateParameter = Output<typeof WhatsappTemplateParameterSchema>;

export const WhatsappTemplateButtonTypeSchema = enumeration('url');
export type WhatsappTemplateButtonType = Output<typeof WhatsappTemplateButtonTypeSchema>;

export const WhatsappTemplateComponentSchema = tagged(
  {
    header: object({
      type: enumeration('header'),
      parameters: array(WhatsappTemplateParameterSchema),
    }),
    body: object({ type: enumeration('body'), parameters: array(WhatsappTemplateParameterSchema) }),
    button: object({
      type: enumeration('button'),
      sub_type: WhatsappTemplateButtonTypeSchema,
      index: text,
      parameters: array(WhatsappTemplateParameterSchema),
    }),
  },
  'type',
);
export type WhatsappTemplateComponent = Output<typeof WhatsappTemplateComponentSchema>;

export const WhatsappTemplateLanguageSchema = object({ code: text });
export type WhatsappTemplateLanguage = Output<typeof WhatsappTemplateLanguageSchema>;

export const WhatsappTemplateSchema = object({
  name: text,
  language: WhatsappTemplateLanguageSchema,
  components: optional(array(WhatsappTemplateComponentSchema)),
});
export type WhatsappTemplate = Output<typeof WhatsappTemplateSchema>;
