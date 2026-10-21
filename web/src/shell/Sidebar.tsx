import { IconNewChatOutline16, IconPanelLeftOutline16, IconGoalOutline16 } from '../ui/icons';
import type { ReactNode } from 'react';
import type { AppRoute } from '../routing/routes';
import { ServiceMenu } from '../services/ServiceMenu';
import markMarkup from '../assets/brand/cma-mark.svg?raw';
import './Sidebar.css';
const markHtml = { __html: markMarkup };

function plainClick(event: React.MouseEvent) {
  return !(event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey);
}

interface SidebarProps {
  readonly collapsed: boolean;
  readonly onToggleCollapsed: () => void;
  readonly account: ReactNode;
  readonly navigation?: ReactNode;
  readonly canCreateAgent?: boolean;
  readonly canCreateTrigger?: boolean;
  readonly onNewAgent?: () => void;
  readonly onNewTrigger?: () => void;
  readonly onHome?: () => void;
  readonly route?: AppRoute;
  readonly onNavigate?: (route: AppRoute) => void;
}

export function Sidebar({
  collapsed,
  onToggleCollapsed,
  account,
  navigation,
  canCreateAgent = false,
  canCreateTrigger = false,
  onNewAgent,
  onNewTrigger,
  onHome,
  route = { kind: 'agent_manager' },
  onNavigate,
}: SidebarProps) {
  return (
    <aside
      className={`sidebar${collapsed ? ' sidebar--collapsed' : ''}`}
      aria-label="Application navigation"
    >
      <div className="sidebar__head">
        <a
          className="sidebar__brand"
          href="/"
          aria-label="CMA home"
          title="ChronoAI Managed Agent"
          onClick={(event) => {
            if (!plainClick(event)) return;
            if (!onHome && !onNavigate) return;
            event.preventDefault();
            if (onHome) onHome();
            else onNavigate?.({ kind: 'home' });
          }}
        >
          <span className="sidebar__mark" aria-hidden="true" dangerouslySetInnerHTML={markHtml} />
        </a>
        <ServiceMenu route={route} collapsed={collapsed} onNavigate={onNavigate ?? (() => {})} />
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
      <div className="sidebar__actions">
        {canCreateAgent ? (
          <button
            type="button"
            className="sidebar__new"
            aria-label="New agent"
            title="New agent"
            onClick={onNewAgent}
          >
            <IconNewChatOutline16 />
            <span>New agent</span>
          </button>
        ) : null}
        {canCreateTrigger ? (
          <button
            type="button"
            className="sidebar__new"
            aria-label="New agent trigger"
            title="New agent trigger"
            onClick={onNewTrigger}
          >
            <IconGoalOutline16 />
            <span>New agent trigger</span>
          </button>
        ) : null}
      </div>
      <div className="sidebar__navigation" hidden={collapsed}>
        {navigation}
      </div>
      <div className="sidebar__account">{account}</div>
    </aside>
  );
}
