export const attachmentLimit = 10;
export const attachmentSizeLimit = 20 * 1024 * 1024;

export interface UploadedAttachment {
  readonly id: string;
  readonly agentId: string;
  readonly name: string;
  readonly path: string;
  readonly size: number;
  readonly mediaType: string;
  readonly sha256: string;
}

export interface DraftAttachment {
  readonly key: string;
  readonly file: File;
  readonly status: 'queued' | 'uploading' | 'ready' | 'error';
  readonly uploaded?: UploadedAttachment;
  readonly error?: string;
}

export type UploadAttachment = (
  file: File,
  options: { readonly idempotencyKey: string; readonly signal: AbortSignal },
) => Promise<UploadedAttachment>;

export const attachmentMarker =
  '\n\nAttached files are available in the sandbox. Read the relevant files at these paths; images can be opened with your image tool:\n';

export interface MessageAttachment {
  readonly name: string;
  readonly path: string;
  readonly media_type: string;
  readonly size_bytes: number;
}

/** Presentation only: malformed or manually edited manifests remain visible verbatim. */
export function splitAttachmentMessage(
  content: string,
): { readonly text: string; readonly files: readonly MessageAttachment[] } | undefined {
  const index = content.lastIndexOf(attachmentMarker);
  if (index < 0) return undefined;
  try {
    const files: unknown = JSON.parse(content.slice(index + attachmentMarker.length));
    if (!Array.isArray(files) || files.length === 0 || files.length > attachmentLimit) return;
    const valid = files.every((file: unknown) => {
      if (typeof file !== 'object' || file === null) return false;
      const value = file as Record<string, unknown>;
      return (
        typeof value.name === 'string' &&
        typeof value.path === 'string' &&
        value.path.startsWith('/home/attachments/') &&
        typeof value.media_type === 'string' &&
        typeof value.size_bytes === 'number' &&
        Number.isSafeInteger(value.size_bytes) &&
        value.size_bytes >= 0
      );
    });
    return valid
      ? { text: content.slice(0, index), files: files as MessageAttachment[] }
      : undefined;
  } catch {
    return undefined;
  }
}

/** The model receives exact server-confirmed paths, never a client-chosen destination. */
export function attachmentMessage(content: string, items: readonly DraftAttachment[]): string {
  if (items.length === 0) return content.trim();
  if (items.some((item) => item.status !== 'ready' || item.uploaded === undefined)) return '';
  const manifest = items.map(({ uploaded }) => ({
    name: uploaded!.name,
    path: uploaded!.path,
    media_type: uploaded!.mediaType,
    size_bytes: uploaded!.size,
  }));
  return `${content.trim() || 'Please review the attached files.'}${attachmentMarker}${JSON.stringify(manifest, null, 2)}`;
}
