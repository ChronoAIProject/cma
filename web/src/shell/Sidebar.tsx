import markMarkup from '../assets/brand/cma-mark-animated.svg?raw';
import { IconPanelLeftOutline16 } from '../ui/icons';
import { BUILD } from './build';
import type { ReactNode } from 'react';
import './Sidebar.css';

interface SidebarProps {
  readonly collapsed: boolean;
  readonly onToggleCollapsed: () => void;
  readonly account: ReactNode;
}

export function Sidebar({ collapsed, onToggleCollapsed, account }: SidebarProps) {
  return (
    <aside className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}`}>
      <div className="sidebar__head">
        <a className="sidebar__brand" href="/" aria-label="CMA home">
          <span
            className="sidebar__mark"
            aria-hidden="true"
            dangerouslySetInnerHTML={{ __html: markMarkup }}
          />
          {collapsed ? null : <span className="sidebar__name">CMA</span>}
        </a>
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
      {collapsed ? null : (
        <span className="sidebar__build" title="Build revision">
          {BUILD}
        </span>
      )}
      <div className="sidebar__space" />
      <div className="sidebar__account">{account}</div>
    </aside>
  );
}
