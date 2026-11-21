import { DocsLink } from '../services/DocsLink';
import { CreationRequestHelp, type CreationRequest } from './CreationRequestHelp';
import { useEffect, useRef, useState } from 'react';
import { DocumentError, type DocumentsClient, type DocumentView } from '../api/documents';
import type { AppRoute } from '../routing/routes';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Modal } from '../ui/Modal';
import { SkeletonRows } from '../ui/Skeleton';
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
  const normalizedQuery = query.trim();
  const [cursor, setCursor] = useState<string>();
  const [next, setNext] = useState<string | null>(null);
  const [items, setItems] = useState<readonly DocumentView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [reload, setReload] = useState(0);
  const [create, setCreate] = useState(false);
  const [title, setTitle] = useState('Untitled');
  const [busy, setBusy] = useState(false);
  const [createError, setCreateError] = useState<{
    message: string;
    status?: number;
    refused: boolean;
  }>();
  const [intent, setIntent] = useState<CreationRequest>();
  const [uncertain, setUncertain] = useState(false);
  const mutation = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => mutation.current?.abort(), []);
  useEffect(() => {
    const request = new AbortController();
    const timer = setTimeout(
      () => {
        setLoading(true);
        void client
          .list({ scope, q: normalizedQuery || undefined, cursor, signal: request.signal })
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
      normalizedQuery ? 200 : 0,
    );
    return () => {
      clearTimeout(timer);
      request.abort();
    };
  }, [client, scope, normalizedQuery, cursor, reload]);
  const resetList = () => {
    setCursor(undefined);
    setNext(null);
    setItems([]);
    setLoading(true);
    setError(undefined);
  };
  const createDocument = async () => {
    if (
      mutation.current ||
      !canCreate ||
      !title.trim() ||
      new TextEncoder().encode(title.trim()).length > 240
    )
      return;
    const retained = intent ?? {
      title: title.trim(),
      key: crypto.randomUUID(),
      startedAt: new Date().toISOString(),
    };
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
        setUncertain(false);
        onNavigate({ kind: 'document', documentId: document.id });
      }
    } catch (cause) {
      if (!request.signal.aborted) {
        const status = cause instanceof DocumentError ? cause.status : undefined;
        const refused = status !== undefined && status >= 400 && status < 500 && status !== 408;
        if (!refused || status === 409) setUncertain(true);
        setCreateError({
          message: cause instanceof Error ? cause.message : 'No confirmation was received.',
          status,
          refused,
        });
      }
    } finally {
      if (!request.signal.aborted) setBusy(false);
      mutation.current = undefined;
    }
  };
  return (
    <section className="documents-page" aria-label="Artifacts">
      <header className="documents-page__header">
        <div>
          <h1>Artifacts</h1>
          <p>Your ideas, brought together.</p>
          <DocsLink view="find-your-outputs">Help finding your outputs</DocsLink>
        </div>
        {canCreate || intent ? (
          <Button variant="primary" icon={<IconPlusOutline16 />} onClick={() => setCreate(true)}>
            {intent ? 'Resume creation' : 'New document'}
          </Button>
        ) : null}
      </header>
      {intent && !create ? (
        <Alert tone="warning" title={`Creation request: ${intent.title}`}>
          {createError?.refused
            ? 'The last attempt was refused.'
            : 'Creation has not been confirmed.'}{' '}
          Choose Resume creation to return to this request.
        </Alert>
      ) : null}
      <div className="documents-page__filters">
        <div className="documents-page__tabs" role="group" aria-label="Artifact ownership">
          {(['all', 'owned', 'shared'] as const).map((value) => (
            <button
              key={value}
              type="button"
              aria-pressed={scope === value}
              onClick={() => {
                if (scope === value) return;
                setScope(value);
                resetList();
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
            const value = event.target.value;
            setQuery(value);
            if (value.trim() !== normalizedQuery) resetList();
          }}
        />
      </div>
      {canCreate && !intent ? (
        <section className="documents-page__create" aria-label="Make something new">
          <p>Make something new</p>
          <button type="button" className="document-template" onClick={() => setCreate(true)}>
            <span>
              <IconListPenOutline16 size={32} />
            </span>
            <span className="document-template__label">
              <strong>Blank document</strong>
              <small>A fresh space for your ideas</small>
            </span>
          </button>
        </section>
      ) : null}
      <h2 className="documents-page__section-title">
        {normalizedQuery
          ? 'Search results'
          : scope === 'shared'
            ? 'Shared documents'
            : 'Your library'}
      </h2>
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
        <SkeletonRows label="Loading artifacts" />
      ) : items.length === 0 && !error ? (
        <div className="documents-page__empty">
          <IconListPenOutline16 size={24} />
          <strong>
            {normalizedQuery ? 'No matching documents' : 'A place for your next idea'}
          </strong>
          <p>
            {normalizedQuery
              ? 'Try a different title or search term.'
              : 'Create a document, or save something your Agent makes.'}
          </p>
        </div>
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
                busy ||
                !canCreate ||
                !title.trim() ||
                new TextEncoder().encode(title.trim()).length > 240
              }
              onClick={() => void createDocument()}
            >
              {busy ? 'Creating…' : intent ? 'Retry creation' : 'Create document'}
            </Button>
          </>
        }
      >
        <div className="document-creation-fields">
          <label className="document-field">
            Title
            <Input
              value={title}
              maxLength={240}
              disabled={busy || intent !== undefined}
              aria-describedby={intent ? 'creation-title-help' : undefined}
              onChange={(event) => setTitle(event.target.value)}
            />
          </label>
          {intent ? <p id="creation-title-help">The title stays with this request.</p> : null}
          {busy ? (
            <Alert tone="info">Sending this creation request. Waiting for confirmation.</Alert>
          ) : null}
          {new TextEncoder().encode(title.trim()).length > 240 ? (
            <Alert tone="warning">Keep the title within 240 bytes.</Alert>
          ) : null}
          {createError && intent ? (
            <>
              <Alert
                tone="error"
                title={createError.refused ? 'Creation request refused' : 'Creation not confirmed'}
              >
                <p>{createError.message}</p>
                {createError.refused ? (
                  <p>
                    This attempt was refused (HTTP {createError.status}). Retry uses the same
                    request.
                  </p>
                ) : (
                  <p>Creation has not been confirmed. Retry uses the same request.</p>
                )}
                {createError.refused && uncertain ? (
                  <p>The original creation is still unconfirmed.</p>
                ) : null}
              </Alert>
              <p>
                You can close this dialog and resume from Artifacts. Keep Artifacts open until
                creation is resolved.
              </p>
              {createError.refused && !uncertain ? (
                <Button
                  onClick={() => {
                    setIntent(undefined);
                    setCreateError(undefined);
                    setUncertain(false);
                  }}
                >
                  Edit as a new request
                </Button>
              ) : null}
              <CreationRequestHelp
                request={intent}
                status={createError.status}
                refused={createError.refused}
              />
            </>
          ) : null}
        </div>
      </Modal>
    </section>
  );
}
