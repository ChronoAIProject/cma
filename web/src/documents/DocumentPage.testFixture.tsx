import { render } from '@testing-library/react';
import { afterEach, beforeEach, vi } from 'vitest';
import type { DocumentsClient, DocumentView } from '../api/documents';
import { DocumentPage } from './DocumentPage';
import { retainDocumentDraft } from './drafts';

export const editorCallbacks = { save: () => {} };
vi.mock('./RichDocumentEditor', () => ({
  default: ({
    source,
    editable,
    onChange,
    onSave,
  }: {
    source: string;
    editable: boolean;
    onChange: (v: string) => void;
    onSave: () => void;
  }) => {
    editorCallbacks.save = onSave;
    return (
      <textarea
        aria-label="Document content"
        value={source}
        disabled={!editable}
        onChange={(event) => onChange(event.target.value)}
      />
    );
  },
}));
export const id = `doc_${'a'.repeat(32)}`;
export const view: DocumentView = {
  id,
  owner_subject: 'owner',
  title: 'Original',
  revision: 3,
  source_revision: 1,
  sha256: 'a'.repeat(64),
  size_bytes: 10,
  created_at: '2026-10-22T00:00:00Z',
  updated_at: '2026-10-22T00:00:00Z',
  origin_agent_id: null,
  access: 'owner',
};
export function client(overrides: Partial<DocumentsClient> = {}): DocumentsClient {
  return {
    read: vi.fn().mockResolvedValue(view),
    source: vi.fn().mockResolvedValue({ id, revision: 1, markdown: '# Original' }),
    comments: vi.fn().mockResolvedValue({ items: [], next: null }),
    ...overrides,
  } as unknown as DocumentsClient;
}
export const page = (api: DocumentsClient) =>
  render(<DocumentPage client={api} id={id} viewer="owner" onNavigate={vi.fn()} />);
beforeEach(() => {
  vi.stubEnv('VITE_SAVED_DOCUMENT_EXPORT_ENABLED', undefined);
  retainDocumentDraft('owner', id);
  window.history.replaceState({}, '', '/');
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});
