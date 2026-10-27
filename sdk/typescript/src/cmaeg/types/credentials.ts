import { SecretSchema } from './primitives.js';
import { array, integer, nullable, object, optional, text, type Output } from '../../schema.js';

export const NativeLarkCredentialsSchema = object({
  app_secret: SecretSchema,
  encrypt_key: SecretSchema,
  verification_token: SecretSchema,
});
export type NativeLarkCredentials = Output<typeof NativeLarkCredentialsSchema>;

export const WhatsappCredentialsSchema = object({
  app_secret: SecretSchema,
  access_token: SecretSchema,
  verify_token: SecretSchema,
});
export type WhatsappCredentials = Output<typeof WhatsappCredentialsSchema>;

export const WhatsappWebhookResponseSchema = object({ accepted: integer, refused: integer });
export type WhatsappWebhookResponse = Output<typeof WhatsappWebhookResponseSchema>;

export const HmacSecretRequestSchema = object({ key_id: text, secret: SecretSchema });
export type HmacSecretRequest = Output<typeof HmacSecretRequestSchema>;

export const CredentialsRequestSchema = object({
  native_lark: optional(nullable(NativeLarkCredentialsSchema)),
  whatsapp: optional(nullable(WhatsappCredentialsSchema)),
  agent_key: SecretSchema,
  channel_key: optional(nullable(SecretSchema)),
  discord_bot_token: optional(nullable(SecretSchema)),
  telegram_bot_token: optional(nullable(SecretSchema)),
  telegram_webhook_secret: optional(nullable(SecretSchema)),
  hmac_secrets: optional(array(HmacSecretRequestSchema)),
});
export type CredentialsRequest = Output<typeof CredentialsRequestSchema>;

export const CredentialPatchSchema = object({
  native_lark: optional(nullable(NativeLarkCredentialsSchema)),
  whatsapp: optional(nullable(WhatsappCredentialsSchema)),
  agent_key: optional(nullable(SecretSchema)),
  channel_key: optional(nullable(SecretSchema)),
  discord_bot_token: optional(nullable(SecretSchema)),
  telegram_bot_token: optional(nullable(SecretSchema)),
  telegram_webhook_secret: optional(nullable(SecretSchema)),
  hmac_secrets: optional(nullable(array(HmacSecretRequestSchema))),
});
export type CredentialPatch = Output<typeof CredentialPatchSchema>;
