import { useEffect, useRef, type ReactNode } from 'react';
import { Alert } from '../../ui/Alert';
import { Button } from '../../ui/Button';
import { IconCloseOutline16 } from '../../ui/icons';
import { Modal } from '../../ui/Modal';
import { FilePath } from './FilePaths';

export function FileExplorerFrame({
  panel,
  embedded = false,
  busy,
  onClose,
  expanded,
  children,
}: {
  readonly panel: boolean;
  readonly embedded?: boolean;
  readonly busy: boolean;
  readonly onClose: () => void;
  readonly expanded?: { title: string; content: ReactNode; onClose: () => void };
  readonly children: ReactNode;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (panel && !embedded) heading.current?.focus({ preventScroll: true });
  }, [panel, embedded]);
  const imageDialog = expanded ? (
    <Modal
      key="image"
      open
      headless
      title={expanded.title}
      placement="image-viewer-dialog"
      onClose={expanded.onClose}
    >
      {expanded.content}
    </Modal>
  ) : null;
  if (!panel)
    return (
      imageDialog ?? (
        <Modal
          key="files"
          open
          title="Files"
          description="Browse the persistent /home/ directory."
          placement="agent-files"
          dismissible={!busy}
          onClose={onClose}
        >
          {children}
        </Modal>
      )
    );
  return (
    <>
      <aside
        className={`agent-files-panel${embedded ? ' agent-files-panel--embedded' : ''}`}
        aria-label="Agent files"
        onKeyDown={(event) => {
          if (event.key === 'Escape' && !event.defaultPrevented && !busy) {
            event.preventDefault();
            onClose();
          }
        }}
      >
        {embedded ? null : (
          <header className="agent-files-panel__header">
            <h2 ref={heading} tabIndex={-1}>
              Files
            </h2>
            <Button
              size="sm"
              variant="ghost"
              icon={<IconCloseOutline16 />}
              aria-label="Close files"
              disabled={busy}
              onClick={onClose}
            />
          </header>
        )}
        <div className="agent-files-panel__body">{children}</div>
      </aside>
      {imageDialog}
    </>
  );
}

export function FileExplorerAlerts({
  error,
  notice,
  downloading,
  confirm,
  onDismissError,
  onDismissNotice,
  onCancel,
  onConfirm,
}: {
  readonly error?: string;
  readonly notice?: string;
  readonly downloading: boolean;
  readonly confirm?: string;
  readonly onDismissError: () => void;
  readonly onDismissNotice: () => void;
  readonly onCancel: () => void;
  readonly onConfirm: () => void;
}) {
  return (
    <>
      {error ? (
        <Alert
          tone="error"
          actions={
            <Button size="sm" onClick={onDismissError}>
              Dismiss
            </Button>
          }
        >
          {error}
        </Alert>
      ) : null}
      {notice ? (
        <Alert
          tone="success"
          actions={
            <Button size="sm" onClick={onDismissNotice}>
              Dismiss
            </Button>
          }
        >
          {notice}
        </Alert>
      ) : null}
      {downloading ? <p role="status">Downloading file…</p> : null}
      {confirm ? (
        <Alert
          tone="warning"
          actions={
            <>
              <Button size="sm" onClick={onCancel}>
                Cancel
              </Button>
              <Button size="sm" variant="danger" onClick={onConfirm}>
                Confirm
              </Button>
            </>
          }
        >
          {confirm}
        </Alert>
      ) : null}
    </>
  );
}

export function FileLocation({
  directory,
  canDelete,
  busy,
  onDelete,
}: {
  readonly directory: string;
  readonly canDelete: boolean;
  readonly busy: boolean;
  readonly onDelete: () => void;
}) {
  return (
    <footer className="agent-files__location">
      <FilePath label="Current directory" path={directory} />
      {directory && canDelete ? (
        <Button size="sm" variant="danger" disabled={busy} onClick={onDelete}>
          Delete folder
        </Button>
      ) : null}
    </footer>
  );
}
