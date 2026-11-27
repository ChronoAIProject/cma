import { useEffect, useRef, useState } from 'react';
import { fileBase64 } from '../api/agentFiles';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { FilePreview, imageType, textContent } from './files/FilePreview';
import { ImagePreview } from './files/ImagePreview';
import { MessageContent } from './MessageContent';
import type { DeliverableSelection } from './previewSelection';
import { previewFormat, usePreviewRead, type PreviewValue } from './previewRead';
import { downloadFailureStatus, statusText } from './AnswerDeliverables';
import { saveDelivery } from './saveDelivery';

export function DeliverablePreview({
  selected,
  onClose,
}: {
  readonly selected: DeliverableSelection;
  readonly onClose: () => void;
}) {
  const reading = usePreviewRead(selected);
  const [failure, setFailure] = useState<PreviewValue>();
  const [downloading, setDownloading] = useState(false);
  const pending = useRef<AbortController | undefined>(undefined);
  const value =
    failure ??
    (reading.state.status === 'ready' && !reading.refreshing ? reading.state.value : undefined);
  const bytes = value?.bytes;
  const root = useRef<HTMLElement>(null);
  const { item, access, answerId } = selected;
  useEffect(() => {
    root.current?.querySelector('button')?.focus({ preventScroll: true });
    return () => pending.current?.abort();
  }, [selected]);
  async function download() {
    if (pending.current) return;
    const abort = new AbortController();
    pending.current = abort;
    setDownloading(true);
    try {
      const downloaded = await access.client.downloadAnswerDeliverable(
        access.agentId,
        answerId,
        item,
        { signal: abort.signal },
      );
      if (!abort.signal.aborted) saveDelivery(downloaded, item.reference.name);
    } catch (error) {
      if (!abort.signal.aborted) setFailure({ status: downloadFailureStatus(error) ?? 'error' });
    } finally {
      if (!abort.signal.aborted) {
        pending.current = undefined;
        setDownloading(false);
      }
    }
  }
  const content = bytes
    ? {
        path: item.reference.name,
        size_bytes: bytes.length,
        sha256: item.reference.sha256,
        content_base64: fileBase64(bytes),
      }
    : undefined;
  const format = previewFormat(item);
  const imageMime = format === 'file' ? imageType(item.reference.name) : undefined;
  const text = content && !imageMime ? textContent(content) : undefined;
  return (
    <aside
      ref={root}
      className="delivery-preview"
      aria-label="Deliverable preview"
      aria-busy={!value && reading.state.status !== 'error'}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && !event.defaultPrevented) {
          event.preventDefault();
          onClose();
        }
      }}
    >
      <header className="delivery-preview__header">
        <h2>{item.reference.name}</h2>
        <Button size="sm" onClick={onClose} aria-label="Close preview">
          Close preview
        </Button>
      </header>
      <div className="delivery-preview__body">
        <p>
          {item.reference.resource.kind === 'document'
            ? `Document revision ${item.reference.resource.revision}`
            : item.reference.resource.kind === 'live_file'
              ? 'Live file'
              : 'Published artifact'}
        </p>
        <div className="delivery-preview__actions">
          <Button
            size="sm"
            aria-label={`Download ${item.reference.name}`}
            disabled={
              downloading ||
              !value ||
              !['ready', 'unchecked', 'version_changed'].includes(value.status)
            }
            onClick={() => void download()}
          >
            {downloading ? 'Downloading…' : 'Download'}
          </Button>
          {reading.state.status === 'error' || (value && value.status !== 'ready') ? (
            <Button
              size="sm"
              onClick={() => {
                setFailure(undefined);
                reading.reload();
              }}
            >
              Retry preview
            </Button>
          ) : null}
        </div>
        {value && value.status !== 'error' && content ? (
          <p role="status">{statusText[value.status]}</p>
        ) : null}
        {reading.state.status === 'error' || value?.status === 'error' ? (
          <p role="status">Preview failed; retry.</p>
        ) : !value ? (
          <p role="status">Loading preview…</p>
        ) : value.tooLarge ? (
          <p role="status">Preview exceeds the 1 MiB limit. Download to view this file.</p>
        ) : value.unsupported ? (
          <p role="status">Preview is not supported; download to view this file.</p>
        ) : !content ? (
          <p role="status">{statusText[value.status as keyof typeof statusText]}</p>
        ) : imageMime ? (
          <DeliverableImagePreview
            name={item.reference.name}
            source={`data:${imageMime};base64,${content.content_base64}`}
          />
        ) : format === 'markdown' && text !== undefined ? (
          <div className="delivery-preview__document">
            <MessageContent content={text} inertResources />
          </div>
        ) : (
          <FilePreview
            file={{
              path: item.reference.name,
              name: item.reference.name,
              kind: 'file',
              size_bytes: item.reference.size_bytes,
              hidden: false,
            }}
            content={content}
            draft={text}
            loading={false}
            busy={downloading}
            readOnly
            hideHeader
            onDraft={() => {}}
            onSave={() => {}}
            onReload={() => {}}
            onDownload={() => void download()}
            onDelete={() => {}}
          />
        )}
      </div>
    </aside>
  );
}

function DeliverableImagePreview({
  name,
  source,
}: {
  readonly name: string;
  readonly source: string;
}) {
  const [expanded, setExpanded] = useState(false);
  const close = () => setExpanded(false);
  return (
    <>
      <ImagePreview name={name} source={source} onToggle={() => setExpanded(true)} />
      {expanded ? (
        <Modal open headless title={name} placement="image-viewer-dialog" onClose={close}>
          <ImagePreview expanded name={name} source={source} onToggle={close} />
        </Modal>
      ) : null}
    </>
  );
}
