import { useState } from 'react';
import type { ReactNode } from 'react';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { Menu, type MenuEntry } from '../ui/Menu';
import {
  IconChevronDownOutline14,
  IconChevronRightOutline14,
  IconEllipsisOutline16,
} from '../ui/icons';

export interface TreeRowAction {
  readonly id: string;
  readonly label: string;
  readonly icon: ReactNode;
  readonly danger?: boolean;
  readonly disabled?: boolean;
  readonly reason?: string;
}

interface TreeRowProps {
  readonly nodeId: string;
  readonly parentId?: string;
  readonly level: 1 | 2 | 3;
  readonly label: string;
  readonly icon: ReactNode;
  readonly meta?: ReactNode;
  readonly route?: AppRoute;
  readonly selected?: boolean;
  readonly expanded?: boolean;
  readonly dimmed?: boolean;
  readonly disabled?: boolean;
  readonly reviving?: boolean;
  /**
   * `line` draws the label and its facts on one row and leaves the facts and
   * the menu key to hover, focus and the current row (`docs/Web_Style.md` §10).
   */
  readonly layout?: 'stacked' | 'line';
  readonly actions?: readonly TreeRowAction[];
  readonly onNavigate: (route: AppRoute) => void;
  readonly onToggle?: () => void;
  readonly onActivate?: () => void;
  readonly onAction?: (id: string) => void;
}

export function TreeRow({
  nodeId,
  parentId,
  level,
  label,
  icon,
  meta,
  route,
  selected = false,
  expanded,
  dimmed = false,
  disabled = false,
  reviving = false,
  layout = 'stacked',
  actions = [],
  onNavigate,
  onToggle,
  onActivate,
  onAction,
}: TreeRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const menuItems: MenuEntry[] = actions.map((action) => ({
    id: action.id,
    label: action.label,
    reason: action.reason,
    icon: action.icon,
    danger: action.danger,
    disabled: action.disabled,
  }));

  const labelContent = (
    <>
      <span className="tree-row__icon" aria-hidden="true">
        {icon}
      </span>
      <span className="tree-row__content">
        <span className="tree-row__name" title={label}>
          {label}
        </span>
        {meta === undefined ? null : <span className="tree-row__meta">{meta}</span>}
      </span>
    </>
  );

  return (
    <div
      className="tree-row"
      role="treeitem"
      tabIndex={0}
      aria-level={level}
      aria-expanded={expanded}
      aria-selected={selected}
      aria-disabled={disabled || undefined}
      data-node-id={nodeId}
      data-parent-id={parentId}
      data-dimmed={dimmed ? '' : undefined}
      data-reviving={reviving ? '' : undefined}
      data-level={level}
      data-layout={layout}
    >
      {onToggle === undefined ? (
        <span className="tree-row__disclosure" />
      ) : (
        <button
          type="button"
          className="tree-row__disclosure"
          tabIndex={-1}
          aria-label={`${expanded === true ? 'Collapse' : 'Expand'} ${label}`}
          onClick={onToggle}
        >
          {expanded === true ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}
        </button>
      )}
      {route === undefined && onToggle === undefined && onActivate === undefined ? (
        <span className="tree-row__label">{labelContent}</span>
      ) : route === undefined ? (
        <button
          type="button"
          className="tree-row__label"
          tabIndex={-1}
          aria-current={selected ? 'page' : undefined}
          onClick={onActivate ?? onToggle}
        >
          {labelContent}
        </button>
      ) : (
        <a
          className="tree-row__label"
          href={disabled ? undefined : routePath(route)}
          role={disabled ? 'link' : undefined}
          aria-disabled={disabled || undefined}
          tabIndex={-1}
          aria-current={selected ? 'page' : undefined}
          onClick={(event) => {
            if (disabled) {
              event.preventDefault();
              return;
            }
            if (
              event.button !== 0 ||
              event.metaKey ||
              event.ctrlKey ||
              event.shiftKey ||
              event.altKey
            ) {
              return;
            }
            event.preventDefault();
            onNavigate(route);
          }}
        >
          {labelContent}
        </a>
      )}
      {actions.length === 0 ? null : (
        <Menu
          open={menuOpen}
          align="end"
          items={menuItems}
          label={`Actions for ${label}`}
          onClose={() => {
            setMenuOpen(false);
          }}
          onSelect={(id) => {
            setMenuOpen(false);
            onAction?.(id);
          }}
          anchor={
            <button
              type="button"
              className="tree-row__menu"
              aria-label={`Options for ${label}`}
              title={`Options for ${label}`}
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              onClick={() => {
                setMenuOpen((open) => !open);
              }}
            >
              <IconEllipsisOutline16 />
            </button>
          }
        />
      )}
    </div>
  );
}
