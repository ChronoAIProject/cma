import { SidebarContext } from '../shell/sidebarContext';
import { useContext, useState, type ReactNode, type Ref } from 'react';
import type { AgentStatus } from '../api/types';
import { statusLabel } from '../vocabulary/status';
import { Button } from '../ui/Button';
import { Menu, type MenuEntry } from '../ui/Menu';
import type { Crumb } from '../ui/PageHeader';
import {
  IconChevronDownOutline14,
  IconEllipsisOutline16,
  IconPanelLeftOutline16,
  IconComputerOutline16,
  IconShareOutline16,
} from '../ui/icons';
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
  filesOpen = false,
  onFilesOpenChange,
  onShare,
  onViewDeletionStatus,
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
  readonly onViewDeletionStatus?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const sidebar = useContext(SidebarContext);
  const remove = moreActions.find((item) => item.id === 'delete_agent');
  const action = (id: string, label: string, reason: string): MenuEntry => {
    const item = [...menu, ...moreActions].find((item) => item.id === id && !('type' in item));
    return item ? { ...item, label } : { id, label, disabled: true, reason };
  };
  const items: MenuEntry[] = [
    action('rename-agent', 'Rename', 'Only the task owner can rename it.'),
    action(
      'update-prompt',
      'Update system prompt',
      'Only the task owner can update its system prompt.',
    ),
    action('update-skills', 'Update skills', 'Only the task owner can update its skills.'),
    action('export_history', 'Export history', 'History export is unavailable with this access.'),
    {
      id: 'deletion-status',
      label: 'View deletion status',
      disabled: !onViewDeletionStatus,
      reason: !onViewDeletionStatus ? 'No deletion is in progress.' : undefined,
    },
    { type: 'separator', id: 'before-delete' },
    remove && !('type' in remove)
      ? { ...remove, label: 'Delete task', danger: true }
      : {
          id: 'delete_agent',
          label: 'Delete task',
          danger: true,
          disabled: true,
          reason: 'Task deletion is unavailable with this access.',
        },
  ];
  return (
    <header className="agent-toolbar">
      <div className="agent-toolbar__identity">
        {sidebar?.collapsed ? (
          <Button
            size="sm"
            icon={<IconPanelLeftOutline16 />}
            aria-label="Show sidebar"
            title="Show sidebar"
            onClick={sidebar.toggle}
          />
        ) : null}
        <h1 ref={headingRef} tabIndex={-1} className="agent-toolbar__heading" aria-label={title}>
          <button
            type="button"
            className="agent-toolbar__title"
            title={`${title} · ${statusLabel(status)}`}
            aria-label={detailsOpen ? 'Hide Agent details' : 'Show Agent details'}
            disabled={detailsUnavailable}
            onClick={() => onDetailsOpenChange(!detailsOpen)}
          >
            <span title={title}>{title}</span>
            <IconChevronDownOutline14 />
          </button>
        </h1>
      </div>
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
            icon={<IconComputerOutline16 />}
            aria-label={filesOpen ? 'Hide Agent computer' : 'Show Agent computer'}
            title={filesOpen ? 'Hide Agent computer' : 'Show Agent computer'}
            aria-expanded={filesOpen}
            aria-controls="agent-files-panel"
            onClick={() => onFilesOpenChange(!filesOpen)}
          >
            Computer
          </Button>
        ) : null}
        <Menu
          open={open}
          align="end"
          label="More Agent actions"
          items={items}
          anchor={
            <Button
              size="sm"
              icon={<IconEllipsisOutline16 />}
              aria-label="More Agent actions"
              title="More"
              aria-haspopup="menu"
              aria-expanded={open}
              onClick={() => setOpen(!open)}
            />
          }
          onClose={() => setOpen(false)}
          onSelect={(id) => {
            setOpen(false);
            if (id === 'deletion-status') onViewDeletionStatus?.();
            else if (id === 'delete_agent' || id === 'export_history') onMoreAction?.(id);
            else onMenuSelect?.(id);
          }}
        />
      </div>
    </header>
  );
}
