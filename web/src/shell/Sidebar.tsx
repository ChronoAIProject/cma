import { IconNewChatOutline16, IconPanelLeftOutline16 } from '../ui/icons';
import type { ReactNode } from 'react';
import './Sidebar.css';

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
}: SidebarProps) {
  return (
    <aside className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}`}>
      <div className={`sidebar__head${scope === undefined ? '' : ' sidebar__head--scoped'}`}>
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
