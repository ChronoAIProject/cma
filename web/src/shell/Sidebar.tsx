import markMarkup from '../assets/brand/cma-mark-animated.svg?raw';
import { IconNewChatOutline16, IconPanelLeftOutline16 } from '../ui/icons';
import { BUILD } from './build';
import type { ReactNode } from 'react';
import './Sidebar.css';

interface SidebarProps {
  readonly collapsed: boolean;
  readonly onToggleCollapsed: () => void;
  readonly account: ReactNode;
  readonly navigation?: ReactNode;
  readonly canCreateAgent?: boolean;
  readonly onNewAgent?: () => void;
}

export function Sidebar({
  collapsed,
  onToggleCollapsed,
  account,
  navigation,
  canCreateAgent = false,
  onNewAgent,
}: SidebarProps) {
  return (
    <aside className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}`}>
      <div className="sidebar__head">
        {canCreateAgent ? (
          <button
            type="button"
            className="sidebar__brand"
            aria-label="New agent"
            title="New agent"
            onClick={onNewAgent}
          >
            <span
              className="sidebar__mark"
              aria-hidden="true"
              dangerouslySetInnerHTML={{ __html: markMarkup }}
            />
            {collapsed ? null : (
              <>
                <span className="sidebar__name">CMA</span>
                <IconNewChatOutline16 className="sidebar__new" />
              </>
            )}
          </button>
        ) : (
          <a className="sidebar__brand" href="/" aria-label="CMA home">
            <span
              className="sidebar__mark"
              aria-hidden="true"
              dangerouslySetInnerHTML={{ __html: markMarkup }}
            />
            {collapsed ? null : <span className="sidebar__name">CMA</span>}
          </a>
        )}
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
      {collapsed ? null : navigation}
      <div className="sidebar__space" />
      {collapsed ? null : (
        <span className="sidebar__build" title="Build revision">
          {BUILD}
        </span>
      )}
      <div className="sidebar__account">{account}</div>
    </aside>
  );
}
