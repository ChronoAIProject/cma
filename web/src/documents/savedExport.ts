import type { DocumentsClient, DocumentView } from '../api/documents';

export interface SavedDocumentDownload {
  readonly blob: Blob;
  readonly filename: string;
}

export class SavedDocumentExportError extends Error {}

export function ownsSavedDocument(document: DocumentView, viewer: string): boolean {
  return viewer.length > 0 && document.access === 'owner' && document.owner_subject === viewer;
}

export async function prepareSavedDocumentExport(
  client: Pick<DocumentsClient, 'read' | 'source'>,
  id: string,
  viewer: string,
  signal: AbortSignal,
): Promise<SavedDocumentDownload> {
  signal.throwIfAborted();
  // Do not carry share-link authority into an owner-only export.
  const document = await client.read(id, { signal });
  signal.throwIfAborted();
  if (!ownsSavedDocument(document, viewer)) {
    throw new SavedDocumentExportError('Only documents you own can be exported.');
  }
  if (
    document.id !== id ||
    typeof document.title !== 'string' ||
    !document.title.trim() ||
    new TextEncoder().encode(document.title).length > 240 ||
    !Number.isSafeInteger(document.source_revision) ||
    document.source_revision < 1 ||
    !Number.isSafeInteger(document.size_bytes) ||
    document.size_bytes < 1 ||
    document.size_bytes > 1024 * 1024 ||
    typeof document.sha256 !== 'string' ||
    !/^[a-f0-9]{64}$/.test(document.sha256)
  ) {
    throw new SavedDocumentExportError('Saved document metadata is incomplete or invalid.');
  }
  const source = await client.source(id, { revision: document.source_revision, signal });
  signal.throwIfAborted();
  if (
    source.id !== id ||
    source.revision !== document.source_revision ||
    typeof source.markdown !== 'string' ||
    source.markdown.length > 1024 * 1024
  ) {
    throw new SavedDocumentExportError('The saved source does not match the requested revision.');
  }
  const bytes = new TextEncoder().encode(source.markdown);
  if (bytes.length !== document.size_bytes) {
    throw new SavedDocumentExportError('The saved source failed its size check.');
  }
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  signal.throwIfAborted();
  const sha256 = Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('');
  if (sha256 !== document.sha256) {
    throw new SavedDocumentExportError('The saved source failed its SHA-256 check.');
  }
  // Keep this allowlist explicit: metadata may gain private fields in later API versions.
  const contents = {
    format: 'cma.saved-document',
    version: 1,
    manifest: {
      title: document.title,
      source_revision: document.source_revision,
      content_type: 'text/markdown;charset=utf-8',
      size_bytes: bytes.length,
      sha256,
    },
    markdown: source.markdown,
  };
  const name = document.title
    .normalize('NFKD')
    .replace(/[^a-zA-Z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80);
  return {
    blob: new Blob([JSON.stringify(contents, null, 2) + '\n'], {
      type: 'application/json;charset=utf-8',
    }),
    filename: `document-${name || 'untitled'}-r${document.source_revision}.json`,
  };
}

export function downloadSavedDocument({ blob, filename }: SavedDocumentDownload): void {
  const anchor = document.createElement('a');
  const url = URL.createObjectURL(blob);
  try {
    anchor.href = url;
    anchor.download = filename;
    document.body.append(anchor);
    anchor.click();
  } finally {
    anchor.remove();
    // Let the browser consume the download before releasing its backing object.
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
}
