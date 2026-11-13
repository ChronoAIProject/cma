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
export interface DocumentDeletionIntent {
  readonly key: string;
  readonly expectedRevision: number;
  readonly title: string;
}
/** Unsaved edits survive SPA navigation in this tab; access is revalidated before restoring them. */
const drafts = new Map<string, DocumentDraft>();
const deletions = new Map<string, DocumentDeletionIntent>();
export const documentDraft = (viewer: string, id: string) => drafts.get(`${viewer}/${id}`);
export function retainDocumentDraft(viewer: string, id: string, draft?: DocumentDraft) {
  const key = `${viewer}/${id}`;
  drafts.delete(key);
  if (draft) drafts.set(key, draft);
  if (drafts.size > 20) drafts.delete(drafts.keys().next().value!);
}

/** Navigation never turns an unconfirmed delete into a new command. REST revalidates its owner. */
export const documentDeletionIntent = (viewer: string, id: string) =>
  deletions.get(`${viewer}/${id}`);
export function retainDocumentDeletionIntent(
  viewer: string,
  id: string,
  intent?: DocumentDeletionIntent,
) {
  const key = `${viewer}/${id}`;
  deletions.delete(key);
  if (intent) deletions.set(key, intent);
  if (deletions.size > 20) deletions.delete(deletions.keys().next().value!);
}
