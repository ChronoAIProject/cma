import { useState, type Ref } from 'react';
import type { AgentStatus } from '../api/types';
import { Button } from '../ui/Button';
import type { MenuEntry } from '../ui/Menu';
import { MoreActions } from '../ui/MoreActions';
import { PageHeader, type Crumb } from '../ui/PageHeader';
import { StatusBadge } from '../ui/Badge';
import { IconInspectOutline12 } from '../ui/icons';
import { IconPinOutline16 } from '../ui/glyphs';
import { useHeaderPin } from './headerPin';
import './AgentHeader.css';

/** The menu row that shows or hides the Agent details column. */
export const AGENT_DETAILS = 'agent-details';

/**
 * The Agent's header, shared by personal and authorized organization
 * conversations — `docs/Web_Style.md` §13.
 *
 * The conversation is the page, so the header stays out of its way: one line,
 * collapsed by default to a 28px strip with the title. Hovering the strip or
 * moving focus into it reveals the full line as an overlay above the
 * conversation, which never moves; Pin keeps it in flow. The heading is in the
 * document either way, so the surface can still move focus to it.
 *
 * "Inside" is judged by React events rather than the DOM, because the More
 * menu's list is portalled to the body: focus or the pointer in that list
 * still counts as being in the header, so the header does not vanish from
 * under an open menu.
 */
export function AgentHeader({
  title,
  status,
  breadcrumb,
  branch,
  headingRef,
  viewerId,
  detailsOpen,
  detailsUnavailable = false,
  onDetailsOpenChange,
  menu = [],
  onMenuSelect,
  moreActions = [],
  onMoreAction,
}: {
  readonly title: string;
  readonly status: AgentStatus;
  /** Where the Agent lives, ending at the Agent itself. */
  readonly breadcrumb: readonly Crumb[];
  /** The Worktree branch the Agent works on, when the viewer may see it. */
  readonly branch?: string;
  readonly headingRef?: Ref<HTMLHeadingElement>;
  /** Whose pin this is; the choice is remembered per viewer. */
  readonly viewerId?: string;
  readonly detailsOpen: boolean;
  readonly detailsUnavailable?: boolean;
  readonly onDetailsOpenChange: (open: boolean) => void;
  /** Rows after the Details toggle, such as the creation origin. */
  readonly menu?: readonly MenuEntry[];
  readonly onMenuSelect?: (id: string) => void;
  /** Stop, export and delete, with any reason they are unavailable. */
  readonly moreActions?: readonly MenuEntry[];
  readonly onMoreAction?: (id: string) => void;
}) {
  const [pinned, setPinned] = useHeaderPin(viewerId);
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const revealed = pinned || hovered || focused;
  const items: MenuEntry[] = [
    {
      id: AGENT_DETAILS,
      label: detailsOpen ? 'Hide Agent details' : 'Show Agent details',
      icon: <IconInspectOutline12 />,
      disabled: detailsUnavailable,
      reason: detailsUnavailable ? 'Details are available once access is confirmed.' : undefined,
    },
    ...(menu.length === 0 ? [] : [{ type: 'separator' as const, id: 'after-details' }, ...menu]),
    // Actions are listed only where they can be carried out.
    ...(moreActions.length === 0 || onMoreAction === undefined
      ? []
      : [{ type: 'separator' as const, id: 'before-actions' }, ...moreActions]),
  ];
  return (
    <div
      className="agent-header-seat"
      data-pinned={pinned ? '' : undefined}
      data-revealed={revealed ? '' : undefined}
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => setHovered(false)}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
    >
      {pinned ? null : (
        <p className="agent-header__strip" aria-hidden="true">
          {title}
        </p>
      )}
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
        actions={
          <>
            <Button
              size="sm"
              variant="ghost"
              icon={<IconPinOutline16 />}
              aria-label={pinned ? 'Unpin header' : 'Pin header'}
              title={pinned ? 'Unpin header' : 'Pin header'}
              aria-pressed={pinned}
              onClick={() => setPinned(!pinned)}
            />
            <MoreActions
              items={items}
              label="More Agent actions"
              onSelect={(id) => {
                const row = menu.find((entry) => entry.id === id);
                if (id === AGENT_DETAILS) onDetailsOpenChange(!detailsOpen);
                else if (row === undefined) onMoreAction?.(id);
                // The menu reports a plain click on a link row instead of following it.
                else if (!('type' in row) && row.href !== undefined)
                  window.location.assign(row.href);
                else onMenuSelect?.(id);
              }}
            />
          </>
        }
      />
    </div>
  );
}
