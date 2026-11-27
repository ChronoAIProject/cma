import { SidebarContext } from '../shell/sidebarContext';
import { useContext, useState, type ReactNode, type Ref } from 'react';
import type { AgentStatus } from '../api/types';
import { AgentMascot } from '../brand/AgentMascot';
import { agentVisual } from '../brand/agentVisual';
import type { SandboxStatusObservation } from '../sandbox/presentation';
import type { ActivityStatus } from './activityStatus';
import { Button } from '../ui/Button';
import { Badge } from '../ui/Badge';
import type { TaskAcceptance } from '@cma/client';
import { acceptanceForTurn, taskAcceptanceStatus } from './taskAcceptance';
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
import { SETUP_ENVIRONMENT_NOTICE } from './setupEnvironment';
import { OPEN_RUNTIME_UPDATE, OPEN_SANDBOX_TO_RESUME } from '../shell/treeActions';

export const AGENT_DETAILS = 'agent-details';

/** The conversation's compact, always-visible toolbar. Panels never own the stream. */
export function AgentHeader({
  title,
  status,
  parentSandbox,
  readOnly = false,
  activityStatus,
  turnId,
  taskAcceptance,
  headingRef,
  detailsOpen,
  detailsUnavailable = false,
  onDetailsOpenChange,
  menu = [],
  onMenuSelect,
  onActionsOpenChange,
  moreActions = [],
  onMoreAction,
  filesOpen = false,
  onFilesOpenChange,
  onShare,
  onViewDeletionStatus,
  onReviewEnvironment,
}: {
  readonly title: string;
  readonly status: AgentStatus;
  readonly parentSandbox?: SandboxStatusObservation;
  readonly readOnly?: boolean;
  readonly activityStatus?: ActivityStatus;
  readonly turnId?: string;
  readonly taskAcceptance?: TaskAcceptance;
  readonly breadcrumb: readonly Crumb[];
  readonly branch?: string;
  readonly headingRef?: Ref<HTMLHeadingElement>;
  readonly viewerId?: string;
  readonly detailsOpen: boolean;
  readonly detailsUnavailable?: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  readonly menu?: readonly MenuEntry[];
  readonly onMenuSelect?: (id: string) => void;
  readonly onActionsOpenChange?: (open: boolean) => void;
  readonly moreActions?: readonly MenuEntry[];
  readonly onMoreAction?: (id: string) => void;
  readonly sandbox?: ReactNode;
  readonly filesOpen?: boolean;
  readonly onFilesOpenChange?: (open: boolean) => void;
  readonly onShare?: () => void;
  readonly progress?: ReactNode;
  readonly onViewDeletionStatus?: () => void;
  readonly onReviewEnvironment?: () => void;
}) {
  const acceptance = status === 'ready' ? acceptanceForTurn(taskAcceptance, turnId) : undefined;
  const visual = agentVisual({
    status: detailsUnavailable ? undefined : status,
    sandbox: parentSandbox,
    task:
      taskAcceptanceStatus(acceptance) ??
      (activityStatus === 'completed' ? undefined : activityStatus),
    acceptance,
    turnId,
  });
  const [open, setMenuOpen] = useState(false);
  const setOpen = (value: boolean) => {
    setMenuOpen(value);
    onActionsOpenChange?.(value);
  };
  const sidebar = useContext(SidebarContext);
  const remove = moreActions.find((item) => item.id === 'delete_agent');
  const removeEnvironment = moreActions.find((item) => item.id === 'delete_sandbox');
  const stop = moreActions.find((item) => item.id === 'stop_agent' && !('type' in item));
  const resume = moreActions.find((item) => item.id === OPEN_SANDBOX_TO_RESUME);
  const runtimeUpdate = moreActions.find((item) => item.id === OPEN_RUNTIME_UPDATE);
  const action = (id: string, label: string, reason: string): MenuEntry => {
    const item = [...menu, ...moreActions].find((item) => item.id === id && !('type' in item));
    return item ? { ...item, label } : { id, label, disabled: true, reason };
  };
  const taskItems: MenuEntry[] = [
    action('rename-agent', 'Rename', 'Only the task owner can rename it.'),
    action(
      'update-prompt',
      'Update system prompt',
      'Only the task owner can update its system prompt.',
    ),
    action('update-skills', 'Update skills', 'Only the task owner can update its skills.'),
    action(
      'export_retained_conversation',
      'Export retained conversation',
      'Conversation export is unavailable with this access.',
    ),
    action(
      'export_history',
      'Export raw history',
      'History export is unavailable with this access.',
    ),
    ...(onReviewEnvironment
      ? [{ id: 'view_environment', label: 'Review environment', reason: SETUP_ENVIRONMENT_NOTICE }]
      : []),
    ...(resume ? [resume] : []),
    ...(runtimeUpdate ? [runtimeUpdate] : []),
    {
      id: 'deletion-status',
      label: 'View deletion status',
      disabled: !onViewDeletionStatus,
      reason: !onViewDeletionStatus ? 'No deletion is in progress.' : undefined,
    },
    ...(stop || remove || removeEnvironment
      ? [{ type: 'separator', id: 'before-delete' } as const]
      : []),
    ...(stop ? [stop] : []),
    ...(remove && !('type' in remove) ? [{ ...remove, label: 'Delete task', danger: true }] : []),
    ...(removeEnvironment ? [removeEnvironment] : []),
  ];
  const taskItemIds = new Set(taskItems.map(({ id }) => id));
  const suppliedItems = menu.filter(({ id }) => !taskItemIds.has(id));
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
            title={title}
            aria-label={detailsOpen ? 'Hide Agent details' : 'Show Agent details'}
            disabled={detailsUnavailable}
            onClick={() => onDetailsOpenChange(!detailsOpen)}
          >
            <span>{title}</span>
            <IconChevronDownOutline14 />
          </button>
        </h1>
      </div>
      <div className="agent-toolbar__status" title={visual.detail}>
        <AgentMascot visual={visual} size={32} />
        {readOnly ? <Badge tone="neutral">Read only</Badge> : null}
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
          placement="agent-toolbar__menu"
          label="More Agent actions"
          items={[...suppliedItems, ...taskItems]}
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
            else if (id === 'view_environment') onReviewEnvironment?.();
            else if (
              id === 'stop_agent' ||
              id === 'delete_agent' ||
              id === 'delete_sandbox' ||
              id === OPEN_SANDBOX_TO_RESUME ||
              id === OPEN_RUNTIME_UPDATE ||
              id === 'export_history' ||
              id === 'export_retained_conversation'
            )
              onMoreAction?.(id);
            else {
              const item = suppliedItems.find((entry) => entry.id === id);
              if (item && !('type' in item) && item.href !== undefined)
                window.location.assign(item.href);
              else onMenuSelect?.(id);
            }
          }}
        />
      </div>
    </header>
  );
}
