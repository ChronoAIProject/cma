import type { ReactNode, Ref } from 'react';
import type { AgentStatus } from '../api/types';
import { Button } from '../ui/Button';
import type { MenuEntry } from '../ui/Menu';
import { MoreActions } from '../ui/MoreActions';
import { PageHeader, type Crumb } from '../ui/PageHeader';
import { StatusBadge } from '../ui/Badge';
import { IconInspectOutline12 } from '../ui/icons';

/** Presentation shared by personal and authorized organization conversations. */
export function AgentHeader({
  title,
  status,
  breadcrumb,
  branch,
  headingRef,
  detailsOpen,
  detailsUnavailable = false,
  onDetailsOpenChange,
  moreActions = [],
  onMoreAction,
  children,
}: {
  readonly title: string;
  readonly status: AgentStatus;
  /** Where the Agent lives, ending at the Agent itself. */
  readonly breadcrumb: readonly Crumb[];
  /** The Worktree branch the Agent works on, when the viewer may see it. */
  readonly branch?: string;
  readonly headingRef?: Ref<HTMLHeadingElement>;
  readonly detailsOpen: boolean;
  readonly detailsUnavailable?: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  /** Stop, export and delete, with any reason they are unavailable. */
  readonly moreActions?: readonly MenuEntry[];
  readonly onMoreAction?: (id: string) => void;
  /** The creation origin and any account prompt, under the title. */
  readonly children?: ReactNode;
}) {
  return (
    <PageHeader
      placement="agent-header"
      breadcrumb={breadcrumb}
      title={title}
      headingRef={headingRef}
      badge={<StatusBadge status={status} />}
      meta={
        branch === undefined || branch === '' ? undefined : (
          <span className="page-header__chip" title={`Worktree branch ${branch}`}>
            {branch}
          </span>
        )
      }
      description={children}
      actions={
        <>
          <Button
            size="sm"
            variant="toolbar"
            icon={<IconInspectOutline12 />}
            aria-label={detailsOpen ? 'Hide Agent details' : 'Show Agent details'}
            title={detailsOpen ? 'Hide Agent details' : 'Show Agent details'}
            aria-pressed={detailsOpen}
            disabled={detailsUnavailable}
            onClick={() => onDetailsOpenChange(!detailsOpen)}
          />
          {onMoreAction === undefined ? null : (
            <MoreActions items={moreActions} onSelect={onMoreAction} label="More Agent actions" />
          )}
        </>
      }
    />
  );
}
