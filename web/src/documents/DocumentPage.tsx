import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import type { DocumentsClient, DocumentView } from '../api/documents';
import { DocumentError } from '../api/documents';
import type { AppRoute } from '../routing/routes';
import { DocumentNavigationGuard } from './DocumentNavigationGuard';
import { Button } from '../ui/Button';
import { Alert } from '../ui/Alert';
import { Modal } from '../ui/Modal';
import { SkeletonRows } from '../ui/Skeleton';
import { usePresence } from '../ui/Presence';
import { IconChevronLeftOutline14, IconShareOutline16, IconDownloadOutline16 } from '../ui/icons';
import { DocumentComments } from './DocumentComments';
import { DocumentSharing } from './DocumentSharing';
import { DocumentSaveNotice } from './DocumentSaveNotice';
import { documentDraft, retainDocumentDraft, type DocumentSaveIntent } from './drafts';
import { readDocumentShareToken, retainDocumentShareToken } from './shareToken';
import {
  downloadSavedDocument,
  ownsSavedDocument,
  prepareSavedDocumentExport,
  SavedDocumentAccessError,
  SavedDocumentExportError,
} from './savedExport';
import { savedDocumentExportEnabled } from './savedExportFeature';
const RichDocumentEditor = lazy(() => import('./RichDocumentEditor'));
interface Loaded {
  readonly document: DocumentView;
  readonly markdown: string;
  readonly baseSourceRevision: number;
}

interface DocumentPageProps {
  readonly client: DocumentsClient;
  readonly id: string;
  readonly viewer: string;
  readonly onNavigate: (route: AppRoute) => void;
}

export function DocumentPage(props: DocumentPageProps) {
  return <DocumentPageContent key={`${props.viewer}/${props.id}`} {...props} />;
}

function DocumentPageContent({ client, id, viewer, onNavigate }: DocumentPageProps) {
  const [shareToken] = useState(() => readDocumentShareToken(id));
  const [loaded, setLoaded] = useState<Loaded>();
  const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0);
  const [title, setTitle] = useState('');
  const [markdown, setMarkdown] = useState('');
  const [commentsOpen, setCommentsOpen] = useState(false);
  const [preview, setPreview] = useState(false);
  const commentsPresence = usePresence(commentsOpen);
  const [shareOpen, setShareOpen] = useState(false);
  const [discardOpen, setDiscardOpen] = useState(false);
  const [conflict, setConflict] = useState(false);
  const [busy, setBusy] = useState(false);
  const [intent, setIntent] = useState<DocumentSaveIntent>();
  const loadRequest = useRef<AbortController | undefined>(undefined);
  const mutation = useRef<AbortController | undefined>(undefined);
  const exportRequest = useRef<AbortController | undefined>(undefined);
  const [exporting, setExporting] = useState<AbortController>();
  const owner = loaded?.document.access === 'owner';
  const dirty = owner && (title !== loaded.document.title || markdown !== loaded.markdown);
  const canChange = owner && !preview && !busy && !conflict;
  const canEdit = canChange;
  const canSave =
    canChange &&
    (intent !== undefined ||
      (dirty &&
        title.trim().length > 0 &&
        new TextEncoder().encode(title.trim()).length <= 240 &&
        markdown.trim().length > 0 &&
        new TextEncoder().encode(markdown).length <= 1024 * 1024));
  const revoke = () => {
    loadRequest.current?.abort();
    mutation.current?.abort();
    exportRequest.current?.abort();
    retainDocumentDraft(viewer, id);
    setLoaded(undefined);
    setTitle('');
    setMarkdown('');
    setIntent(undefined);
    setBusy(false);
    setShareOpen(false);
    setDiscardOpen(false);
  };
  useEffect(() => {
    retainDocumentShareToken(id);
  }, [id]);
  useEffect(() => () => mutation.current?.abort(), []);
  useEffect(() => () => exportRequest.current?.abort(), [client, id, viewer]);
  useEffect(() => {
    const request = new AbortController();
    loadRequest.current = request;
    void (async () => {
      const document = await client.read(id, { shareToken, signal: request.signal });
      if (request.signal.aborted) return;
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
    return () => {
      request.abort();
      if (loadRequest.current === request) loadRequest.current = undefined;
    };
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
  const save = async () => {
    if (mutation.current || !loaded || !canSave) return;
    const retained = intent ?? {
      key: crypto.randomUUID(),
      body: { title: title.trim(), markdown, expected_revision: loaded.document.revision },
    };
    const request = new AbortController();
    mutation.current = request;
    const submittedTitle = intent ? retained.body.title : title;
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
      setTitle((current) => (current === submittedTitle ? document.title : current));
      // FileReader may have inserted an image while this snapshot was saving.
      // Keep that newer local source; the acknowledged snapshot is the new baseline.
      setIntent(undefined);
    } catch (cause) {
      if (!request.signal.aborted) {
        if (cause instanceof DocumentError && [401, 403, 404].includes(cause.status)) revoke();
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
  const exportSaved = async () => {
    if (
      (exportRequest.current && !exportRequest.current.signal.aborted) ||
      !savedDocumentExportEnabled() ||
      !loaded ||
      loaded.document.id !== id ||
      !ownsSavedDocument(loaded.document, viewer)
    )
      return;
    const request = new AbortController();
    exportRequest.current = request;
    setExporting(request);
    setError(undefined);
    try {
      const result = await prepareSavedDocumentExport(client, id, viewer, request.signal);
      if (request.signal.aborted) return;
      downloadSavedDocument(result);
      console.info('Saved document export download started.');
    } catch (cause) {
      if (!request.signal.aborted) {
        const accessLost = cause instanceof SavedDocumentAccessError;
        if (accessLost) revoke();
        const failure = accessLost ? cause.cause : cause;
        console.warn('Saved document export failed.', {
          status: failure instanceof DocumentError ? failure.status : undefined,
          accessLost,
        });
        setError(
          cause instanceof SavedDocumentExportError
            ? cause.message
            : 'The saved document could not be exported. Reload the document and try again.',
        );
      }
    } finally {
      if (exportRequest.current === request) {
        exportRequest.current = undefined;
        setExporting(undefined);
      }
    }
  };
  return (
    <section className="document-workspace document-page" aria-label="Document">
      <DocumentNavigationGuard
        id={id}
        protectedChanges={Boolean(owner && (dirty || intent))}
        saving={intent !== undefined}
      />
      <header className="document-workspace__header document-page__header">
        <div className="document-page__heading">
          <Button
            size="sm"
            variant="ghost"
            icon={<IconChevronLeftOutline14 />}
            aria-label="Back to artifacts"
            onClick={() => onNavigate({ kind: 'artifacts' })}
          />
          {loaded ? (
            <input
              className="document-title document-page__title"
              aria-label="Document title"
              value={title}
              maxLength={240}
              disabled={!canEdit}
              onChange={(event) => setTitle(event.target.value)}
            />
          ) : (
            <h1>Opening document</h1>
          )}
        </div>
        {loaded ? (
          <p className="document-page__subtitle">
            Artifacts · {owner ? (preview ? 'You’re previewing' : 'You’re editing') : 'Read only'}
          </p>
        ) : null}
        <div className="document-page__controls">
          <span
            className="document-workspace__save-status document-page__save-status"
            role="status"
            data-dirty={dirty || undefined}
            data-unconfirmed={(intent !== undefined && !busy) || undefined}
          >
            {busy
              ? 'Saving…'
              : conflict
                ? 'Version changed'
                : intent
                  ? 'Save not confirmed'
                  : dirty
                    ? 'Unsaved changes'
                    : loaded
                      ? 'Saved'
                      : ''}
            {preview && dirty && !busy && !conflict ? (
              <>
                {' · '}
                <span>Switch to Edit to save</span>
              </>
            ) : null}
          </span>
          <div className="document-page__actions">
            {owner ? (
              <div className="document-workspace__mode" role="group" aria-label="Document mode">
                <Button size="sm" aria-pressed={!preview} onClick={() => setPreview(false)}>
                  Edit
                </Button>
                <Button size="sm" aria-pressed={preview} onClick={() => setPreview(true)}>
                  Preview
                </Button>
              </div>
            ) : null}
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
            {savedDocumentExportEnabled() &&
            loaded?.document.id === id &&
            ownsSavedDocument(loaded.document, viewer) ? (
              <Button
                size="sm"
                icon={<IconDownloadOutline16 />}
                disabled={(exporting && !exporting.signal.aborted) || busy || intent !== undefined}
                onClick={() => void exportSaved()}
              >
                {exporting && !exporting.signal.aborted
                  ? 'Exporting saved JSON…'
                  : 'Export saved JSON'}
              </Button>
            ) : null}
            {owner && !intent ? (
              <Button size="sm" variant="primary" disabled={!canSave} onClick={() => void save()}>
                Save
              </Button>
            ) : null}
          </div>
        </div>
      </header>
      {owner && new TextEncoder().encode(title.trim()).length > 240 ? (
        <Alert tone="warning">Keep the document title within 240 bytes.</Alert>
      ) : null}
      {error && !intent ? (
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
      {owner && intent ? (
        <DocumentSaveNotice
          text={markdown}
          busy={busy}
          canRetry={canSave}
          error={error}
          onRetry={() => void save()}
        />
      ) : null}
      <div
        className="document-workspace__body document-page__body"
        data-comments={commentsOpen || undefined}
      >
        <div className="document-workspace__canvas document-page__canvas">
          {loaded ? (
            <Suspense
              fallback={
                <div className="document-editor__paper">
                  <SkeletonRows rows={6} label="Opening editor" />
                </div>
              }
            >
              <RichDocumentEditor
                key={`${id}:${revision}`}
                source={markdown}
                editable={canEdit}
                onChange={setMarkdown}
                onSave={() => void save()}
              />
            </Suspense>
          ) : !error ? (
            <div className="document-editor__paper">
              <SkeletonRows rows={6} label="Loading document" />
            </div>
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
          key={loaded.document.id}
          client={client}
          document={loaded.document}
          open={shareOpen}
          onUpdated={updateMetadata}
          onClose={() => setShareOpen(false)}
        />
      ) : null}
      {loaded ? (
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
      ) : null}
    </section>
  );
}
