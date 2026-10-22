import { useEffect, useRef, useState } from 'react';
import { DocumentError, type DocumentsClient, type DocumentView } from '../api/documents';
import type { AppRoute } from '../routing/routes';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import { IconPlusOutline16, IconListPenOutline16 } from '../ui/icons';

export function DocumentRegistry({
  client,
  canCreate,
  onNavigate,
}: {
  readonly client: DocumentsClient;
  readonly canCreate: boolean;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [scope, setScope] = useState<'all' | 'owned' | 'shared'>('all');
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState<string>();
  const [next, setNext] = useState<string | null>(null);
  const [items, setItems] = useState<readonly DocumentView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [reload, setReload] = useState(0);
  const [create, setCreate] = useState(false);
  const [title, setTitle] = useState('Untitled');
  const [busy, setBusy] = useState(false);
  const [createError, setCreateError] = useState<string>();
  const [intent, setIntent] = useState<{ title: string; key: string }>();
  const mutation = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => mutation.current?.abort(), []);
  useEffect(() => {
    const request = new AbortController();
    const timer = setTimeout(
      () => {
        setLoading(true);
        void client
          .list({ scope, q: query.trim() || undefined, cursor, signal: request.signal })
          .then(
            (page) => {
              if (request.signal.aborted) return;
              setItems((previous) =>
                cursor
                  ? [
                      ...previous,
                      ...page.items.filter((item) => !previous.some((old) => old.id === item.id)),
                    ]
                  : page.items,
              );
              setNext(page.next);
              setError(undefined);
            },
            (cause) => {
              if (!request.signal.aborted)
                setError(cause instanceof Error ? cause.message : 'Artifacts could not be loaded.');
            },
          )
          .finally(() => {
            if (!request.signal.aborted) setLoading(false);
          });
      },
      query ? 200 : 0,
    );
    return () => {
      clearTimeout(timer);
      request.abort();
    };
  }, [client, scope, query, cursor, reload]);
  const createDocument = async () => {
    if (
      mutation.current ||
      !canCreate ||
      !title.trim() ||
      new TextEncoder().encode(title.trim()).length > 240
    )
      return;
    const retained = intent ?? { title: title.trim(), key: crypto.randomUUID() };
    const request = new AbortController();
    mutation.current = request;
    setIntent(retained);
    setBusy(true);
    setCreateError(undefined);
    try {
      const document = await client.create(
        { title: retained.title, markdown: '# Untitled\n' },
        { idempotencyKey: retained.key, signal: request.signal },
      );
      if (!request.signal.aborted) {
        setIntent(undefined);
        onNavigate({ kind: 'document', documentId: document.id });
      }
    } catch (cause) {
      if (!request.signal.aborted) {
        if (cause instanceof DocumentError && cause.status >= 400 && cause.status < 500)
          setIntent(undefined);
        setCreateError(
          cause instanceof Error
            ? cause.message
            : 'Creation could not be confirmed. Retry this request.',
        );
      }
    } finally {
      if (!request.signal.aborted) setBusy(false);
      mutation.current = undefined;
    }
  };
  return (
    <section className="documents-page" aria-label="Artifacts">
      <header className="documents-page__header">
        <h1>Artifacts</h1>
        {canCreate ? (
          <Button icon={<IconPlusOutline16 />} onClick={() => setCreate(true)}>
            New document
          </Button>
        ) : null}
      </header>
      <div className="documents-page__filters">
        <div className="documents-page__tabs" role="group" aria-label="Artifact ownership">
          {(['all', 'owned', 'shared'] as const).map((value) => (
            <button
              key={value}
              type="button"
              aria-pressed={scope === value}
              onClick={() => {
                setScope(value);
                setCursor(undefined);
                setItems([]);
              }}
            >
              {{ all: 'All', owned: 'Yours', shared: 'Shared with you' }[value]}
            </button>
          ))}
        </div>
        <Input
          aria-label="Search artifacts"
          placeholder="Search artifacts"
          type="search"
          value={query}
          maxLength={160}
          onChange={(event) => {
            setQuery(event.target.value);
            setCursor(undefined);
          }}
        />
      </div>
      {canCreate ? (
        <section className="documents-page__create" aria-label="Make something new">
          <p>Make something new</p>
          <button type="button" className="document-template" onClick={() => setCreate(true)}>
            <span>
              <IconListPenOutline16 size={32} />
            </span>
            <strong>Docs</strong>
            <small>Write, share and collaborate</small>
          </button>
        </section>
      ) : null}
      {error ? (
        <Alert
          tone="error"
          actions={
            <Button size="sm" onClick={() => setReload((value) => value + 1)}>
              Retry
            </Button>
          }
        >
          {error}
        </Alert>
      ) : null}
      <div className="document-gallery" aria-busy={loading}>
        {items.map((document) => (
          <a
            key={document.id}
            className="document-card"
            href={`/artifacts/${document.id}`}
            onClick={(event) => {
              if (event.button || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey)
                return;
              event.preventDefault();
              onNavigate({ kind: 'document', documentId: document.id });
            }}
          >
            <div className="document-card__preview" aria-hidden="true">
              <IconListPenOutline16 size={24} />
              <span>{document.title}</span>
            </div>
            <div className="document-card__info">
              <strong>{document.title}</strong>
              <small>
                {document.access === 'owner' ? 'Your document' : 'Shared with you'} · Updated{' '}
                {new Date(document.updated_at).toLocaleDateString()}
              </small>
            </div>
          </a>
        ))}
      </div>
      {loading ? (
        <p role="status">Loading artifacts…</p>
      ) : items.length === 0 && !error ? (
        <p>No documents yet{query ? ' match your search' : ''}.</p>
      ) : null}
      {next ? (
        <Button disabled={loading} onClick={() => setCursor(next)}>
          Load more
        </Button>
      ) : null}
      <Modal
        open={create}
        title="New document"
        dismissible={!busy}
        onClose={() => setCreate(false)}
        footer={
          <>
            <Button disabled={busy} onClick={() => setCreate(false)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={
                busy || !title.trim() || new TextEncoder().encode(title.trim()).length > 240
              }
              onClick={() => void createDocument()}
            >
              {busy ? 'Creating…' : intent ? 'Retry creation' : 'Create document'}
            </Button>
          </>
        }
      >
        <label className="document-field">
          Title
          <Input
            value={title}
            maxLength={240}
            disabled={busy || intent !== undefined}
            onChange={(event) => setTitle(event.target.value)}
          />
        </label>
        {new TextEncoder().encode(title.trim()).length > 240 ? (
          <Alert tone="warning">Keep the title within 240 bytes.</Alert>
        ) : null}
        {createError ? <Alert tone="error">{createError}</Alert> : null}
      </Modal>
    </section>
  );
}
