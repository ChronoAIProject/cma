import { useState, type RefObject } from 'react';
import { DocumentError, type DocumentsClient, type DocumentView } from '../api/documents';
import { Button } from '../ui/Button';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { readableError } from '../vocabulary/errors';
import { documentDeletionIntent, retainDocumentDeletionIntent } from './drafts';

export function DocumentDeletion({
  client,
  id,
  viewer,
  document,
  mutationRef,
  disabled,
  onPending,
  onDeleting,
  onConflict,
  onDeleted,
}: {
  readonly client: DocumentsClient;
  readonly id: string;
  readonly viewer: string;
  readonly document?: DocumentView;
  readonly mutationRef: RefObject<AbortController | undefined>;
  readonly disabled: boolean;
  readonly onPending: (value: boolean) => void;
  readonly onDeleting: (value: boolean) => void;
  readonly onConflict: (message: string) => void;
  readonly onDeleted: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [intent, setIntent] = useState(() => documentDeletionIntent(viewer, id));
  const newDeletionDisabled = disabled || document?.access !== 'owner';
  const blocked = busy || (!intent && newDeletionDisabled);
  const remove = async () => {
    if (mutationRef.current || blocked) return;
    const replaying = intent !== undefined;
    const retained =
      intent ??
      (document && !newDeletionDisabled
        ? {
            key: crypto.randomUUID(),
            expectedRevision: document.revision,
            title: document.title,
          }
        : undefined);
    if (!retained) return;
    const request = new AbortController();
    mutationRef.current = request;
    setIntent(retained);
    retainDocumentDeletionIntent(viewer, id, retained);
    setBusy(true);
    onPending(true);
    onDeleting(true);
    setError(undefined);
    try {
      await client.deleteDocument(id, {
        expectedRevision: retained.expectedRevision,
        idempotencyKey: retained.key,
        signal: request.signal,
      });
      if (request.signal.aborted) return;
      setIntent(undefined);
      retainDocumentDeletionIntent(viewer, id);
      onPending(false);
      setOpen(false);
      onDeleted();
    } catch (cause) {
      if (request.signal.aborted) return;
      const conflict =
        cause instanceof DocumentError &&
        cause.status === 409 &&
        (!cause.code || cause.code === 'document_revision_conflict');
      // A refusal of a replay cannot undo an earlier, unconfirmed deletion.
      const refused =
        cause instanceof DocumentError &&
        (conflict ||
          (!replaying &&
            ((cause.status >= 400 && cause.status < 500) ||
              cause.code === 'document_deletion_unavailable')));
      if (refused) {
        setIntent(undefined);
        retainDocumentDeletionIntent(viewer, id);
        onPending(false);
      }
      if (conflict) {
        setOpen(false);
        onConflict(
          'The document changed before it could be deleted. Your draft is retained; reload the latest version before trying again.',
        );
      } else
        setError(
          refused
            ? readableError(cause, 'This document could not be deleted. Try again later.')
            : cause instanceof DocumentError && cause.code === 'document_deletion_unavailable'
              ? `${readableError(cause, 'Document deletion is unavailable.')} The earlier deletion result is still unconfirmed. Retry the same delete request.`
              : 'Deletion could not be confirmed. Retry the same delete request to check its result.',
        );
    } finally {
      mutationRef.current = undefined;
      if (!request.signal.aborted) {
        setBusy(false);
        onDeleting(false);
      }
    }
  };
  return (
    <>
      <Button size="sm" disabled={blocked} onClick={() => setOpen(true)}>
        {intent && !busy ? 'Retry delete' : 'Delete document'}
      </Button>
      <RiskConfirmation
        open={open}
        title="Delete this document?"
        description={`Deleting “${intent?.title ?? document?.title ?? 'this document'}” removes its saved versions, comments and sharing access. Unsaved changes will be discarded. This cannot be undone. Storage capacity is released only after cleanup finishes.`}
        acknowledgeLabel="I understand that this document and its saved versions will be permanently deleted."
        confirmLabel={busy ? 'Deleting…' : intent ? 'Retry delete' : 'Delete document'}
        busy={busy}
        confirmDisabled={blocked}
        dismissible={!busy}
        failure={error}
        onCancel={() => setOpen(false)}
        onConfirm={() => void remove()}
      />
    </>
  );
}
