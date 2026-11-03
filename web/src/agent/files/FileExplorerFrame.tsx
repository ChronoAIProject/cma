import { useEffect, useRef, type ReactNode } from 'react';
import { Button } from '../../ui/Button';
import { IconCloseOutline16 } from '../../ui/icons';
import { Modal } from '../../ui/Modal';

export function FileExplorerFrame({
  panel,
  embedded = false,
  busy,
  onClose,
  children,
}: {
  readonly panel: boolean;
  readonly embedded?: boolean;
  readonly busy: boolean;
  readonly onClose: () => void;
  readonly children: ReactNode;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (panel && !embedded) heading.current?.focus({ preventScroll: true });
  }, [panel, embedded]);
  if (!panel)
    return (
      <Modal
        open
        title="Files"
        description="Browse the persistent /home/ directory."
        placement="agent-files"
        dismissible={!busy}
        onClose={onClose}
      >
        {children}
      </Modal>
    );
  return (
    <aside
      className="agent-files-panel"
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
  );
}
