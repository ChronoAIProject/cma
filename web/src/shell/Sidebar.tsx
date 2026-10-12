import markMarkup from '../assets/brand/cma-mark-animated.svg?raw';
import { IconNewChatOutline16, IconPanelLeftOutline16 } from '../ui/icons';
import { BUILD } from './build';
import type { ReactNode } from 'react';
import './Sidebar.css';

// Stable identity keeps hierarchy refreshes from replacing and reanimating the SVG.
const markHtml = { __html: markMarkup };

interface SidebarProps {
  readonly collapsed: boolean;
  readonly onToggleCollapsed: () => void;
  readonly account: ReactNode;
  readonly navigation?: ReactNode;
  readonly scope?: ReactNode;
  readonly canCreateAgent?: boolean;
  readonly onNewAgent?: () => void;
  readonly onHome?: () => void;
}

export function Sidebar({
  collapsed,
  onToggleCollapsed,
  account,
  navigation,
  scope,
  canCreateAgent = false,
  onNewAgent,
  onHome,
}: SidebarProps) {
  return (
    <aside className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}`}>
      <div className={`sidebar__head${scope === undefined ? '' : ' sidebar__head--scoped'}`}>
        <a
          className="sidebar__brand"
          href="/"
          aria-label="CMA home"
          onClick={(event) => {
            if (
              onHome === undefined ||
              event.button !== 0 ||
              event.metaKey ||
              event.ctrlKey ||
              event.shiftKey ||
              event.altKey
            )
              return;
            event.preventDefault();
            onHome();
          }}
        >
          <span className="sidebar__mark" aria-hidden="true" dangerouslySetInnerHTML={markHtml} />
          {collapsed || scope !== undefined ? null : <span className="sidebar__name">CMA</span>}
        </a>
        {collapsed ? null : (
          <span
            className="sidebar__build"
            aria-label={`Build ${BUILD}`}
            title={`Build revision: ${BUILD}`}
          >
            {BUILD.replace(/^trureturing-/, '')}
          </span>
        )}
        {collapsed ? null : scope}
        {canCreateAgent ? (
          <button
            type="button"
            className="sidebar__new"
            aria-label="New agent"
            title="New agent"
            onClick={onNewAgent}
          >
            <IconNewChatOutline16 />
          </button>
        ) : null}
        <button
          type="button"
          className="sidebar__toggle"
          aria-label={collapsed ? 'Show navigation' : 'Hide navigation'}
          aria-expanded={!collapsed}
          onClick={onToggleCollapsed}
        >
          <IconPanelLeftOutline16 />
        </button>
      </div>
      <div className="sidebar__navigation">{collapsed ? null : navigation}</div>
      <div className="sidebar__account">{account}</div>
    </aside>
  );
}
