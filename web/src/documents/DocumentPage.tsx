import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import type { DocumentsClient, DocumentView } from '../api/documents';
import { DocumentError } from '../api/documents';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { Alert } from '../ui/Alert';
import { Modal } from '../ui/Modal';
import { usePresence } from '../ui/Presence';
import { IconChevronLeftOutline14, IconShareOutline16, IconDownloadOutline16 } from '../ui/icons';
import { DocumentComments } from './DocumentComments';
import { DocumentSharing } from './DocumentSharing';
import { documentDraft, retainDocumentDraft, type DocumentSaveIntent } from './drafts';
import { readDocumentShareToken, retainDocumentShareToken } from './shareToken';
const RichDocumentEditor = lazy(() => import('./RichDocumentEditor'));
interface Loaded {
  readonly document: DocumentView;
  readonly markdown: string;
  readonly baseSourceRevision: number;
}

export function DocumentPage({
  client,
  id,
  viewer,
  onNavigate,
}: {
  readonly client: DocumentsClient;
  readonly id: string;
  readonly viewer: string;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [shareToken] = useState(() => readDocumentShareToken(id));
  const [loaded, setLoaded] = useState<Loaded>();
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [title, setTitle] = useState('');
  const [markdown, setMarkdown] = useState('');
  const [commentsOpen, setCommentsOpen] = useState(false);
  const commentsPresence = usePresence(commentsOpen);
  const [shareOpen, setShareOpen] = useState(false);
  const [discardOpen, setDiscardOpen] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [busy, setBusy] = useState(false);
  const [intent, setIntent] = useState<DocumentSaveIntent>();
  const mutation = useRef<AbortController | undefined>(undefined);
  const owner = loaded?.document.access === 'owner';
  const dirty = owner && (title !== loaded.document.title || markdown !== loaded.markdown);
  useEffect(() => {
    retainDocumentShareToken(id);
  }, [id]);
  useEffect(() => () => mutation.current?.abort(), []);
  useEffect(() => {
    const request = new AbortController();
    void (async () => {
      const document = await client.read(id, { shareToken, signal: request.signal });
      const source = await client.source(id, {
        shareToken,
        revision: document.source_revision,
        signal: request.signal,
      });
      if (request.signal.aborted) return;
      const retained = document.access === 'owner' ? documentDraft(viewer, id) : undefined;
      if (document.access !== 'owner') retainDocumentDraft(viewer, id);
      setLoaded({
        document,
        markdown: source.markdown,
        baseSourceRevision: retained?.sourceRevision ?? document.source_revision,
      });
      setTitle(retained?.title ?? document.title);
      setMarkdown(retained?.markdown ?? source.markdown);
      setIntent(retained?.saveIntent);
      setConflict(
        retained !== undefined &&
          !retained.saveIntent &&
          retained.sourceRevision !== document.source_revision,
      );
      setError(undefined);
    })().catch((cause) => {
      if (!request.signal.aborted)
        setError(cause instanceof Error ? cause.message : 'This document could not be loaded.');
    });
    return () => request.abort();
  }, [client, id, viewer, shareToken, revision]);
  useEffect(() => {
    if (!loaded || !owner) return;
    retainDocumentDraft(
      viewer,
      id,
      dirty || intent
        ? { title, markdown, sourceRevision: loaded.baseSourceRevision, saveIntent: intent }
        : undefined,
    );
  }, [viewer, id, loaded, owner, dirty, title, markdown, intent]);
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);
  const save = async () => {
    if (
      mutation.current ||
      !loaded ||
      !owner ||
      conflict ||
      !title.trim() ||
      new TextEncoder().encode(title.trim()).length > 240 ||
      !markdown.trim() ||
      new TextEncoder().encode(markdown).length > 1024 * 1024
    )
      return;
    const retained = intent ?? {
      key: crypto.randomUUID(),
      body: { title: title.trim(), markdown, expected_revision: loaded.document.revision },
    };
    const request = new AbortController();
    mutation.current = request;
    setBusy(true);
    setIntent(retained);
    setError(undefined);
    try {
      const document = await client.update(id, retained.body, {
        signal: request.signal,
        idempotencyKey: retained.key,
      });
      if (request.signal.aborted) return;
      setLoaded({
        document,
        markdown: retained.body.markdown,
        baseSourceRevision: document.source_revision,
      });
      setTitle(document.title);
      // FileReader may have inserted an image while this snapshot was saving.
      // Keep that newer local source; the acknowledged snapshot is the new baseline.
      setIntent(undefined);
      retainDocumentDraft(viewer, id);
    } catch (cause) {
      if (!request.signal.aborted) {
        if (cause instanceof DocumentError && cause.status >= 400 && cause.status < 500) {
          setIntent(undefined);
          if (cause.status === 409) setConflict(true);
        }
        setError(
          cause instanceof Error
            ? cause.message
            : 'Save could not be confirmed. Retry the same save.',
        );
      }
    } finally {
      mutation.current = undefined;
      if (!request.signal.aborted) setBusy(false);
    }
  };
  const updateMetadata = (document: DocumentView) => {
    if (loaded && document.source_revision !== loaded.document.source_revision) {
      setConflict(true);
      setError(
        'This document changed. Your draft is retained; reload the latest version before saving.',
      );
      return;
    }
    setLoaded((current) => (current ? { ...current, document } : current));
  };
  const reloadLatest = () => {
    setLoaded(undefined);
    setRevision((value) => value + 1);
  };
  const download = () => {
    const url = URL.createObjectURL(new Blob([markdown], { type: 'text/markdown;charset=utf-8' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = `${
      Array.from(title, (character) =>
        character.charCodeAt(0) < 32 || '/\\'.includes(character) ? '-' : character,
      )
        .join('')
        .slice(0, 100) || 'document'
    }.md`;
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  return (
    <section className="document-workspace" aria-label="Document">
      <header className="document-workspace__header">
        <Button
          size="sm"
          variant="ghost"
          icon={<IconChevronLeftOutline14 />}
          aria-label="Back to artifacts"
          onClick={() => onNavigate({ kind: 'artifacts' })}
        />
        {loaded ? (
          <input
            className="document-title"
            aria-label="Document title"
            value={title}
            maxLength={240}
            readOnly={!owner || busy || intent !== undefined}
            onChange={(event) => setTitle(event.target.value)}
          />
        ) : (
          <h1>Opening document</h1>
        )}
        <span className="document-workspace__save-status" role="status">
          {busy
            ? 'Saving…'
            : conflict
              ? 'Version changed'
              : dirty
                ? 'Unsaved changes'
                : loaded
                  ? 'Saved'
                  : ''}
        </span>
        <Button
          size="sm"
          variant="ghost"
          aria-expanded={commentsOpen}
          onClick={() => setCommentsOpen(!commentsOpen)}
        >
          Comments
        </Button>
        {owner ? (
          <Button
            size="sm"
            icon={<IconShareOutline16 />}
            disabled={busy || intent !== undefined}
            onClick={() => setShareOpen(true)}
          >
            Share
          </Button>
        ) : null}
        {owner ? (
          <Button
            size="sm"
            variant="primary"
            disabled={
              busy ||
              (!dirty && !intent) ||
              conflict ||
              !title.trim() ||
              new TextEncoder().encode(title.trim()).length > 240 ||
              !markdown.trim() ||
              new TextEncoder().encode(markdown).length > 1024 * 1024
            }
            onClick={() => void save()}
          >
            {intent && !busy ? 'Retry save' : 'Save'}
          </Button>
        ) : null}
      </header>
      {owner && new TextEncoder().encode(title.trim()).length > 240 ? (
        <Alert tone="warning">Keep the document title within 240 bytes.</Alert>
      ) : null}
      {error ? (
        <Alert
          tone="error"
          actions={
            <>
              <Button
                size="sm"
                onClick={() => (loaded && dirty ? setDiscardOpen(true) : reloadLatest())}
              >
                Reload latest
              </Button>
              {loaded ? (
                <Button size="sm" icon={<IconDownloadOutline16 />} onClick={download}>
                  Download draft
                </Button>
              ) : null}
            </>
          }
        >
          {error}
        </Alert>
      ) : null}
      {conflict && !error ? (
        <Alert
          tone="warning"
          actions={
            <Button size="sm" onClick={() => setDiscardOpen(true)}>
              Reload latest
            </Button>
          }
        >
          This document changed while you were away. Your unsaved draft is retained; download it
          before loading the latest version.
        </Alert>
      ) : null}
      <div className="document-workspace__body" data-comments={commentsOpen || undefined}>
        <div className="document-workspace__canvas">
          {loaded ? (
            <Suspense fallback={<p role="status">Opening editor…</p>}>
              <RichDocumentEditor
                key={`${id}:${revision}`}
                source={markdown}
                editable={owner && !busy && intent === undefined && !conflict}
                onChange={setMarkdown}
                onSave={() => void save()}
              />
            </Suspense>
          ) : !error ? (
            <p role="status">Loading document…</p>
          ) : null}
        </div>
        {loaded && commentsPresence.present ? (
          <DocumentComments
            client={client}
            exiting={commentsPresence.exiting}
            document={loaded.document}
            viewer={viewer}
            shareToken={shareToken}
            onUpdated={updateMetadata}
            onClose={() => setCommentsOpen(false)}
          />
        ) : null}
      </div>
      {loaded && owner ? (
        <DocumentSharing
          client={client}
          document={loaded.document}
          open={shareOpen}
          onUpdated={updateMetadata}
          onClose={() => setShareOpen(false)}
        />
      ) : null}
      <Modal
        open={discardOpen}
        title="Load the latest document?"
        onClose={() => setDiscardOpen(false)}
        footer={
          <>
            <Button onClick={download}>Download draft</Button>
            <Button
              variant="danger"
              onClick={() => {
                retainDocumentDraft(viewer, id);
                setDiscardOpen(false);
                setIntent(undefined);
                setLoaded(undefined);
                setRevision((value) => value + 1);
              }}
            >
              Discard draft and reload
            </Button>
          </>
        }
      >
        <p>Your unsaved changes will be replaced by the latest saved version.</p>
      </Modal>
    </section>
  );
}
