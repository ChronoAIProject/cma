import { DocsLink } from '../services/DocsLink';
import { CreationRequestHelp } from './CreationRequestHelp';
import { creationRefused, readCreationRecovery, writeCreationRecovery } from './creationRecovery';
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
  viewer,
  canCreate,
  onNavigate,
}: {
  readonly client: DocumentsClient;
  readonly viewer: string;
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
  const [saved, setSaved] = useState(() => readCreationRecovery(viewer));
  const [title, setTitle] = useState(saved.recovery?.request.title ?? 'Untitled');
  const [busy, setBusy] = useState(false);
  const [createError, setCreateError] = useState<string>();
  const { recovery, error: recoveryError } = saved;
  const intent = recovery?.request;
  const refused = creationRefused(recovery?.status);
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
      recoveryError ||
      !title.trim() ||
      new TextEncoder().encode(title.trim()).length > 240
    )
      return;
    const retained = intent ?? {
      title: title.trim(),
      key: crypto.randomUUID(),
      startedAt: new Date().toISOString(),
    };
    const pending = { request: retained, uncertain: true };
    const storageError = writeCreationRecovery(viewer, pending);
    if (storageError) {
      setSaved({ recovery, error: storageError });
      return;
    }
    const request = new AbortController();
    mutation.current = request;
    setSaved({ recovery: pending });
    setBusy(true);
    setCreateError(undefined);
    try {
      const document = await client.create(
        { title: retained.title, markdown: '# Untitled\n' },
        { idempotencyKey: retained.key, signal: request.signal },
      );
      if (!request.signal.aborted) {
        const storageError = writeCreationRecovery(viewer);
        if (storageError) {
          setSaved({ recovery: pending, error: storageError });
          return;
        }
        setSaved({});
        onNavigate({ kind: 'document', documentId: document.id });
      }
    } catch (cause) {
      if (!request.signal.aborted) {
        const status = cause instanceof DocumentError ? cause.status : undefined;
        const failed = {
          request: retained,
          uncertain: !!recovery?.uncertain || !creationRefused(status) || status === 409,
          status,
        };
        const storageError = writeCreationRecovery(viewer, failed);
        setSaved({
          recovery: storageError ? pending : failed,
          error: storageError,
        });
        setCreateError(cause instanceof Error ? cause.message : 'No confirmation was received.');
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
        {(canCreate && !recoveryError) || intent ? (
          <Button variant="primary" icon={<IconPlusOutline16 />} onClick={() => setCreate(true)}>
            {intent ? 'Resume creation' : 'New document'}
          </Button>
        ) : null}
      </header>
      {recoveryError && !create ? <Alert tone="error">{recoveryError}</Alert> : null}
      {intent && !create ? (
        <Alert tone="warning" title={`Creation request: ${intent.title}`}>
          {refused ? 'The last attempt was refused.' : 'Creation has not been confirmed.'} Choose
          Resume creation to return to this request.
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
      {canCreate && !intent && !recoveryError ? (
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
                !!recoveryError ||
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
              data-initial-focus="true"
              aria-describedby={intent ? 'creation-title-help' : undefined}
              onFocus={(event) => {
                if (event.currentTarget.value === 'Untitled') event.currentTarget.select();
              }}
              onChange={(event) => setTitle(event.target.value)}
            />
          </label>
          {intent ? <p id="creation-title-help">The title stays with this request.</p> : null}
          {recoveryError ? <Alert tone="error">{recoveryError}</Alert> : null}
          {busy ? (
            <Alert tone="info">Sending this creation request. Waiting for confirmation.</Alert>
          ) : null}
          {new TextEncoder().encode(title.trim()).length > 240 ? (
            <Alert tone="warning">Keep the title within 240 bytes.</Alert>
          ) : null}
          {intent && !busy ? (
            <>
              <Alert
                tone="error"
                title={refused ? 'Creation request refused' : 'Creation not confirmed'}
              >
                {createError ? <p>{createError}</p> : null}
                {refused ? (
                  <p>
                    This attempt was refused (HTTP {recovery?.status}). Retry uses the same request.
                  </p>
                ) : (
                  <p>Creation has not been confirmed. Retry uses the same request.</p>
                )}
                {refused && recovery?.uncertain ? (
                  <p>The original creation is still unconfirmed.</p>
                ) : null}
              </Alert>
              <p>You can return to Artifacts in this browser tab to resume this request.</p>
              {refused && !recovery?.uncertain && !recoveryError ? (
                <Button
                  onClick={() => {
                    const storageError = writeCreationRecovery(viewer);
                    if (storageError) {
                      setSaved({ recovery, error: storageError });
                      return;
                    }
                    setSaved({});
                    setCreateError(undefined);
                  }}
                >
                  Edit as a new request
                </Button>
              ) : null}
              <CreationRequestHelp request={intent} status={recovery?.status} refused={refused} />
            </>
          ) : null}
        </div>
      </Modal>
    </section>
  );
}
