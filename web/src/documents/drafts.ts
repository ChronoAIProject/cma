export interface DocumentSaveIntent {
  readonly key: string;
  readonly body: { title: string; markdown: string; expected_revision: number };
}
export interface DocumentDraft {
  readonly title: string;
  readonly markdown: string;
  readonly sourceRevision: number;
  readonly saveIntent?: DocumentSaveIntent;
}
/** Unsaved edits survive SPA navigation in this tab; access is revalidated before restoring them. */
const drafts = new Map<string, DocumentDraft>();
export const documentDraft = (viewer: string, id: string) => drafts.get(`${viewer}/${id}`);
export function retainDocumentDraft(viewer: string, id: string, draft?: DocumentDraft) {
  const key = `${viewer}/${id}`;
  drafts.delete(key);
  if (draft) drafts.set(key, draft);
  if (drafts.size > 20) drafts.delete(drafts.keys().next().value!);
}
