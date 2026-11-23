import {
  attachmentLimit,
  attachmentSizeLimit,
  describeAttachment,
  type AttachmentDescription,
  type DraftAttachment,
  type UploadedAttachment,
} from './attachments';

export interface StoredAttachment {
  readonly key: string;
  readonly description: AttachmentDescription;
  readonly uploaded?: UploadedAttachment;
}

function bounded(value: unknown, maximum: number): value is string {
  return typeof value === 'string' && new TextEncoder().encode(value).length <= maximum;
}

function description(value: unknown): value is AttachmentDescription {
  if (typeof value !== 'object' || value === null) return false;
  const item = value as AttachmentDescription;
  return (
    bounded(item.name, 1024) &&
    !!item.name &&
    Number.isSafeInteger(item.size) &&
    item.size >= 0 &&
    item.size <= attachmentSizeLimit &&
    bounded(item.mediaType, 127)
  );
}

function uploaded(value: unknown, agentId: string): value is UploadedAttachment {
  if (!description(value)) return false;
  const item = value as UploadedAttachment;
  return (
    typeof item.id === 'string' &&
    /^att_[a-f0-9]{64}$/.test(item.id) &&
    item.agentId === agentId &&
    /^[A-Za-z0-9_-][A-Za-z0-9._-]{0,119}$/.test(item.name) &&
    item.path === `/home/attachments/${agentId}/${item.id}/${item.name}` &&
    /^[A-Za-z0-9!#$&^_.+-]+\/[A-Za-z0-9!#$&^_.+-]+$/.test(item.mediaType) &&
    typeof item.sha256 === 'string' &&
    /^[a-f0-9]{64}$/.test(item.sha256)
  );
}

export function storeAttachments(items: readonly DraftAttachment[]): readonly StoredAttachment[] {
  return items.map((item) => ({
    key: item.key,
    description: describeAttachment(item),
    ...(item.status === 'ready' && item.uploaded !== undefined
      ? {
          uploaded: {
            id: item.uploaded.id,
            agentId: item.uploaded.agentId,
            name: item.uploaded.name,
            path: item.uploaded.path,
            size: item.uploaded.size,
            mediaType: item.uploaded.mediaType,
            sha256: item.uploaded.sha256,
          },
        }
      : {}),
  }));
}

export function restoreAttachments(value: unknown, agentId: string): readonly DraftAttachment[] {
  if (!Array.isArray(value) || value.length > attachmentLimit)
    throw new Error('Invalid attachments');
  const keys = new Set<string>();
  return value.map((entry: unknown) => {
    if (typeof entry !== 'object' || entry === null) throw new Error('Invalid attachment');
    const item = entry as StoredAttachment;
    if (
      !bounded(item.key, 256) ||
      !item.key ||
      keys.has(item.key) ||
      !description(item.description)
    )
      throw new Error('Invalid attachment description');
    keys.add(item.key);
    const ready = uploaded(item.uploaded, agentId) && item.uploaded.size === item.description.size;
    return {
      key: item.key,
      description: item.description,
      status: ready ? 'ready' : 'reselect',
      ...(ready
        ? { uploaded: item.uploaded }
        : { error: 'Reselect this file or remove it before sending.' }),
    };
  });
}
