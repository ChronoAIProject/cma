import { customUsesHmac, object, records, text, type Fields } from './model';

export function hmacSecretMessage(value: unknown): string | undefined {
  const secret = text(value);
  if (!secret) return 'Enter the HMAC secret as standard base64.';
  if (secret.length > 4096) return 'Use at most 4,096 base64 characters.';
  try {
    if (btoa(atob(secret)) === secret) return undefined;
  } catch {
    // The message never includes the supplied secret.
  }
  return 'Use standard base64 with its required padding and no spaces or line breaks.';
}

export function hmacKeyIdMessage(value: unknown): string | undefined {
  const size = new TextEncoder().encode(text(value)).length;
  return size === 0 || size > 512 ? 'Use a key ID containing 1–512 UTF-8 bytes.' : undefined;
}

export function hmacCredentialIdMessage(
  value: unknown,
  rows: readonly Fields[],
  index: number,
): string | undefined {
  return (
    hmacKeyIdMessage(value) ??
    (rows.some((row, other) => other !== index && text(row.key_id) === text(value))
      ? 'Use a different key ID for each HMAC credential.'
      : undefined)
  );
}

export function hmacReferenceMessage(
  value: unknown,
  credentials: Fields,
  create: boolean,
): string | undefined {
  const invalid = hmacKeyIdMessage(value);
  if (invalid) return invalid;
  if (!create && credentials.replace_hmac !== true) return undefined;
  return records(credentials.hmac_secrets).some((row) => text(row.key_id) === text(value))
    ? undefined
    : 'Add an HMAC credential with this exact key ID, or choose an ID from the supplied credentials.';
}

export function validateHmacCredentials(draft: Fields, values: Fields, create: boolean): void {
  // An omitted edit retains the server's secret list; its contents are not returned to the UI.
  if (!create && values.replace_hmac !== true) return;
  const rows = records(values.hmac_secrets);
  for (const [index, row] of rows.entries()) {
    const keyMessage = hmacCredentialIdMessage(row.key_id, rows, index);
    if (keyMessage) throw new Error(`/credentials/hmac_secrets/${index}/key_id: ${keyMessage}`);
    const secretMessage = hmacSecretMessage(row.secret);
    if (secretMessage)
      throw new Error(`/credentials/hmac_secrets/${index}/secret: ${secretMessage}`);
  }
  for (const [index, source] of records(draft.sources).entries()) {
    if (draft.connection_ref || !customUsesHmac(source)) continue;
    const message = hmacReferenceMessage(object(source.auth).key_id, values, create);
    if (message) throw new Error(`/sources/${index}/auth/key_id: ${message}`);
  }
}
