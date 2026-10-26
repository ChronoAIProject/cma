import { array, enumeration, object, optional, tagged, text, type Output } from '../../schema.js';

export const WhatsappTemplateParameterSchema = tagged(
  {
    text: object({ type: enumeration('text'), text }),
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
