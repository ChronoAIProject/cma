/** URL-only native media authoring. The gateway still validates and fetches media. */
export const outboundMediaKinds = ['image', 'file', 'audio', 'video'] as const;
export type OutboundMediaKind = (typeof outboundMediaKinds)[number];
export interface OutboundNativeAttachment {
  readonly kind: OutboundMediaKind;
  readonly source: { readonly type: 'url'; readonly url: string };
  readonly filename?: string;
  readonly mime_type?: string;
  readonly caption?: string;
}
export interface OutboundMessage {
  readonly text?: string;
  readonly native_attachments?: readonly OutboundNativeAttachment[];
}
export interface OutboundMediaDraft {
  readonly id: string;
  readonly kind: OutboundMediaKind;
  readonly url: string;
  readonly filename: string;
  readonly mime: string;
  readonly caption: string;
}
export const maxOutboundMedia = 4;
const bytes = (value: string) => new TextEncoder().encode(value).length;
const forbidden = /[\p{Cc}\u200e\u200f\u202a-\u202e\u2066-\u2069]/u;
const mime = /^[a-z0-9][a-z0-9!#$&^_.+-]{0,126}\/[a-z0-9][a-z0-9!#$&^_.+-]{0,126}$/;

export function validOutboundMediaUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      /^https:\/\//i.test(value) &&
      url.protocol === 'https:' &&
      !!url.hostname &&
      !url.username &&
      !url.password &&
      !value.includes('#') &&
      !/[\s\\]/u.test(value) &&
      !forbidden.test(value) &&
      bytes(value) <= 8192
    );
  } catch {
    return false;
  }
}
export function outboundAttachmentError(item: OutboundNativeAttachment): string | undefined {
  if (!outboundMediaKinds.includes(item.kind)) return 'Choose an image, file, audio or video.';
  if (item.source?.type !== 'url' || !validOutboundMediaUrl(item.source.url))
    return 'Enter an HTTPS URL without a username, password, fragment or spaces.';
  for (const [label, value, limit] of [
    ['Filename', item.filename, 255],
    ['MIME type', item.mime_type, 255],
    ['Caption', item.caption, 1024],
  ] as const) {
    if (
      value !== undefined &&
      (typeof value !== 'string' || !value || bytes(value) > limit || forbidden.test(value))
    )
      return `${label} must contain 1–${limit} bytes without control characters.`;
  }
  if (item.filename && (/[\\/]/.test(item.filename) || ['.', '..'].includes(item.filename)))
    return 'Filename must be a name without a folder path.';
  if (item.mime_type && !mime.test(item.mime_type))
    return 'Enter a lowercase MIME type such as image/png, without parameters.';
  return undefined;
}
export function nativeAttachments(
  items: readonly OutboundMediaDraft[],
): OutboundNativeAttachment[] {
  return items.map((item) => ({
    kind: item.kind,
    source: { type: 'url', url: item.url },
    ...(item.filename ? { filename: item.filename } : {}),
    ...(item.mime ? { mime_type: item.mime } : {}),
    ...(item.caption ? { caption: item.caption } : {}),
  }));
}
export function composeOutboundMessage(
  text: string,
  items: readonly OutboundMediaDraft[],
): OutboundMessage {
  if (!items.length) return { text };
  return { ...(text ? { text } : {}), native_attachments: nativeAttachments(items) };
}
/** Validate retained bodies without reconstructing them or changing request bytes. */
export function validStoredOutboundMessage(
  value: unknown,
  version: number,
): value is OutboundMessage {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const message = value as OutboundMessage;
  if (Object.keys(value).some((key) => !['text', 'native_attachments'].includes(key))) return false;
  if (message.text !== undefined && typeof message.text !== 'string') return false;
  if (version === 1)
    return typeof message.text === 'string' && message.native_attachments === undefined;
  const items = message.native_attachments;
  return (
    version === 2 &&
    Array.isArray(items) &&
    items.length > 0 &&
    items.length <= maxOutboundMedia &&
    items.every((item: unknown) => {
      if (!item || typeof item !== 'object' || Array.isArray(item)) return false;
      const attachment = item as OutboundNativeAttachment;
      const source = attachment.source;
      return (
        Object.keys(item).every((key) =>
          ['kind', 'source', 'filename', 'mime_type', 'caption'].includes(key),
        ) &&
        !!source &&
        typeof source === 'object' &&
        !Array.isArray(source) &&
        Object.keys(source).every((key) => ['type', 'url'].includes(key)) &&
        typeof source.url === 'string' &&
        !outboundAttachmentError(attachment)
      );
    })
  );
}
