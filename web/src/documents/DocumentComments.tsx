import { useEffect, useRef, useState } from 'react';
import type { DocumentsClient, DocumentView, DocumentComment } from '../api/documents';
import { DocumentError } from '../api/documents';
import { Button } from '../ui/Button';
import { IconCloseOutline16 } from '../ui/icons';
import { Alert } from '../ui/Alert';
import { presenceAttributes } from '../ui/Presence';

export function DocumentComments({
  client,
  exiting = false,
  document,
  viewer,
  shareToken,
  onUpdated,
  onClose,
}: {
  readonly client: DocumentsClient;
  readonly exiting?: boolean;
  readonly document: DocumentView;
  readonly viewer: string;
  readonly shareToken?: string;
  readonly onUpdated: (document: DocumentView) => void;
  readonly onClose: () => void;
}) {
  const [comments, setComments] = useState<readonly DocumentComment[]>([]);
  const [text, setText] = useState('');
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  const [cursor, setCursor] = useState<string>();
  const [next, setNext] = useState<string | null>(null);
  const [intent, setIntent] = useState<{ key: string; text: string; revision: number }>();
  const request = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => request.current?.abort(), []);
  useEffect(() => {
    const read = new AbortController();
    void client
      .comments(document.id, { shareToken, cursor, signal: read.signal })
      .then(
        (page) => {
          if (read.signal.aborted) return;
          setComments((old) =>
            cursor
              ? [
                  ...old,
                  ...page.items.filter((item) => !old.some((comment) => comment.id === item.id)),
                ]
              : page.items,
          );
          setNext(page.next);
          setError(undefined);
        },
        (cause) => {
          if (!read.signal.aborted)
            setError(cause instanceof Error ? cause.message : 'Comments could not be loaded.');
        },
      )
      .finally(() => {
        if (!read.signal.aborted) setLoading(false);
      });
    return () => read.abort();
  }, [client, document.id, shareToken, cursor, reload]);
  const post = async () => {
    if (
      request.current ||
      document.access === 'read' ||
      !text.trim() ||
      new TextEncoder().encode(text.trim()).length > 8192
    )
      return;
    const retained = intent ?? {
      key: crypto.randomUUID(),
      text: text.trim(),
      revision: document.source_revision,
    };
    const mutation = new AbortController();
    request.current = mutation;
    setBusy(true);
    setIntent(retained);
    setError(undefined);
    try {
      const value = await client.comment(
        document.id,
        { body: retained.text, document_revision: retained.revision },
        { idempotencyKey: retained.key, signal: mutation.signal, shareToken },
      );
      if (mutation.signal.aborted) return;
      onUpdated(value.document);
      setIntent(undefined);
      setText('');
      setCursor(undefined);
      setReload((value) => value + 1);
    } catch (cause) {
      if (!mutation.signal.aborted) {
        if (cause instanceof DocumentError && cause.status >= 400 && cause.status < 500)
          setIntent(undefined);
        setError(
          cause instanceof Error
            ? cause.message
            : 'Comment could not be confirmed. Retry the same request.',
        );
      }
    } finally {
      request.current = undefined;
      if (!mutation.signal.aborted) setBusy(false);
    }
  };
  return (
    <aside
      className="document-comments"
      aria-label="Document comments"
      {...presenceAttributes(exiting)}
    >
      <header>
        <h2>Comments</h2>
        <Button
          size="sm"
          variant="ghost"
          icon={<IconCloseOutline16 />}
          aria-label="Close comments"
          onClick={onClose}
        />
      </header>
      <div className="document-comments__list">
        {loading ? (
          <p role="status">Loading comments…</p>
        ) : comments.length === 0 ? (
          <p>No comments yet.</p>
        ) : null}
        {comments.map((comment) => (
          <article key={comment.id} className="document-comment">
            <header>
              <strong>{comment.author_subject === viewer ? 'You' : comment.author_subject}</strong>
              <small>Version {comment.document_revision}</small>
            </header>
            <p>{comment.body}</p>
            <time dateTime={comment.created_at}>
              {new Date(comment.created_at).toLocaleString()}
            </time>
          </article>
        ))}
        {next ? (
          <Button size="sm" onClick={() => setCursor(next)}>
            More comments
          </Button>
        ) : null}
        {error ? (
          <Alert
            tone="error"
            actions={
              <Button size="sm" onClick={() => setReload((value) => value + 1)}>
                Retry loading
              </Button>
            }
          >
            {error}
          </Alert>
        ) : null}
      </div>
      {document.access === 'read' ? (
        <p className="document-comments__notice">You have view access to this document.</p>
      ) : (
        <form
          className="document-comments__form"
          onSubmit={(event) => {
            event.preventDefault();
            void post();
          }}
        >
          <textarea
            aria-label="Add a comment"
            placeholder="Add a comment…"
            value={text}
            maxLength={4000}
            disabled={busy || intent !== undefined}
            onChange={(event) => setText(event.target.value)}
          />
          {new TextEncoder().encode(text.trim()).length > 8192 ? (
            <p role="alert">Keep your comment within 8 KiB.</p>
          ) : null}
          <Button
            type="submit"
            size="sm"
            variant="primary"
            disabled={busy || !text.trim() || new TextEncoder().encode(text.trim()).length > 8192}
          >
            {busy ? 'Posting…' : intent ? 'Retry comment' : 'Comment'}
          </Button>
        </form>
      )}
    </aside>
  );
}
