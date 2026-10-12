import { ValidationError } from '../errors.js';

export interface WebhookHeaders {
  readonly 'webhook-id': string;
  readonly 'webhook-timestamp': string;
  readonly 'webhook-signature': string;
}

/** Sign exact body bytes using the UTF-8 secret string, without base64 decoding it. */
export async function signWebhook(
  secret: string,
  id: string,
  timestamp: string,
  body: string,
): Promise<WebhookHeaders> {
  if (!/^[\x21-\x7e]{1,512}$/.test(id) || !/^\d{1,20}$/.test(timestamp))
    throw new ValidationError('webhook id or timestamp is invalid');
  const encoder = new TextEncoder();
  const keyBytes = encoder.encode(secret);
  if (keyBytes.byteLength === 0 || keyBytes.byteLength > 4096)
    throw new ValidationError('webhook secret must contain 1–4096 UTF-8 bytes');
  const key = await globalThis.crypto.subtle.importKey(
    'raw',
    keyBytes,
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  );
  const signature = new Uint8Array(
    await globalThis.crypto.subtle.sign('HMAC', key, encoder.encode(`${id}.${timestamp}.${body}`)),
  );
  return {
    'webhook-id': id,
    'webhook-timestamp': timestamp,
    'webhook-signature': `v1,${btoa(String.fromCharCode(...signature))}`,
  };
}
