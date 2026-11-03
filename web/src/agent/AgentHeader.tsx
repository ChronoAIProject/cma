import { useState, type ReactNode, type Ref } from 'react';
import type { AgentStatus } from '../api/types';
import { statusLabel } from '../vocabulary/status';
import { Button } from '../ui/Button';
import { Menu, type MenuEntry } from '../ui/Menu';
import type { Crumb } from '../ui/PageHeader';
import { IconChevronDownOutline14, IconFolderOpenOutline16, IconShareOutline16 } from '../ui/icons';
import './AgentHeader.css';

export const AGENT_DETAILS = 'agent-details';

/** The conversation's compact, always-visible toolbar. Panels never own the stream. */
export function AgentHeader({
  title,
  status,
  headingRef,
  detailsOpen,
  detailsUnavailable = false,
  onDetailsOpenChange,
  menu = [],
  onMenuSelect,
  moreActions = [],
  onMoreAction,
  sandbox,
  filesOpen = false,
  onFilesOpenChange,
  onShare,
  progress,
}: {
  readonly title: string;
  readonly status: AgentStatus;
  readonly breadcrumb: readonly Crumb[];
  readonly branch?: string;
  readonly headingRef?: Ref<HTMLHeadingElement>;
  readonly viewerId?: string;
  readonly detailsOpen: boolean;
  readonly detailsUnavailable?: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly menu?: readonly MenuEntry[];
  readonly onMenuSelect?: (id: string) => void;
  readonly moreActions?: readonly MenuEntry[];
  readonly onMoreAction?: (id: string) => void;
  readonly sandbox?: ReactNode;
  readonly filesOpen?: boolean;
  readonly onFilesOpenChange?: (open: boolean) => void;
  readonly onShare?: () => void;
  readonly progress?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const items: MenuEntry[] = [
    ...menu,
    ...(menu.length ? [{ type: 'separator' as const, id: 'before-details' }] : []),
    {
      id: AGENT_DETAILS,
      label: detailsOpen ? 'Hide Agent details' : 'Show Agent details',
      disabled: detailsUnavailable,
      reason: detailsUnavailable ? 'Details are available once access is confirmed.' : undefined,
    },
    ...(moreActions.length === 0 || onMoreAction === undefined
      ? []
      : [{ type: 'separator' as const, id: 'before-actions' }, ...moreActions]),
  ];
  return (
    <header className="agent-toolbar">
      <div className="agent-toolbar__identity">
        <h1 ref={headingRef} tabIndex={-1} className="agent-toolbar__heading" aria-label={title}>
          <Menu
            open={open}
            align="start"
            label="More Agent actions"
            anchor={
              <button
                type="button"
                className="agent-toolbar__title"
                aria-label="More Agent actions"
                title={`${title} · ${statusLabel(status)}`}
                aria-haspopup="menu"
                aria-expanded={open}
                onClick={() => setOpen(!open)}
              >
                <span title={title}>{title}</span>
                <IconChevronDownOutline14 />
              </button>
            }
            items={items}
            onClose={() => setOpen(false)}
            onSelect={(id) => {
              setOpen(false);
              const row = menu.find((entry) => entry.id === id);
              if (id === AGENT_DETAILS) onDetailsOpenChange(!detailsOpen);
              else if (row === undefined) onMoreAction?.(id);
              else if (!('type' in row) && row.href !== undefined) window.location.assign(row.href);
              else onMenuSelect?.(id);
            }}
          />
        </h1>
        {sandbox}
      </div>
      {progress}
      <div className="agent-toolbar__actions">
        {onShare ? (
          <Button size="sm" variant="ghost" icon={<IconShareOutline16 />} onClick={onShare}>
            Share
          </Button>
        ) : null}
        {onFilesOpenChange ? (
          <Button
            size="sm"
            variant="ghost"
            icon={<IconFolderOpenOutline16 />}
            aria-label={filesOpen ? 'Hide Agent computer' : 'Show Agent computer'}
            title={filesOpen ? 'Hide Agent computer' : 'Show Agent computer'}
            aria-expanded={filesOpen}
            aria-controls="agent-files-panel"
            onClick={() => onFilesOpenChange(!filesOpen)}
          >
            Computer
          </Button>
        ) : null}
      </div>
    </header>
  );
}
