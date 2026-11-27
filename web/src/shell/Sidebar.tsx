import { IconPanelLeftOutline16, IconPlusOutline16, IconSearchOutline16 } from '../ui/icons';
import { useState, type ReactNode } from 'react';
import { NavigationSettingsContext } from './sidebarContext';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { ServiceMenu } from '../services/ServiceMenu';
import { pixelBrand } from '../assets/brand/pixel/identity';
import { useBrandTheme } from '../brand/AgentMascot';
import './Sidebar.css';

/** One navigation row under the action row, such as Triggers. */
export interface SidebarLink {
  readonly id: string;
  readonly label: string;
  readonly icon: ReactNode;
  readonly route: AppRoute;
  readonly current: boolean;
}

/**
 * New agent, shown whenever the caller could ever create one.
 *
 * A reason makes it disabled rather than absent (§12.4): in Shared or
 * Organization scope the control used to vanish, which read as the feature
 * having gone away rather than as "switch scope first".
 */
export interface SidebarNewAgent {
  readonly current?: boolean;
  readonly onCreate: () => void;
  readonly label?: string;
  readonly icon?: ReactNode;
  readonly unavailableReason?: string;
}

interface SidebarProps {
  readonly collapsed: boolean;
  readonly onToggleCollapsed?: () => void;
  readonly account: ReactNode;
  readonly navigation?: ReactNode;
  readonly newAgent?: SidebarNewAgent;
  /** Opens the command menu. Absent where there is nothing to search. */
  readonly onSearch?: () => void;
  readonly links?: readonly SidebarLink[];
  readonly onHome?: () => void;
  readonly route?: AppRoute;
  readonly onNavigate?: (route: AppRoute) => void;
}

const SHORTCUT_HINT =
  typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.userAgent) ? '⌘K' : 'Ctrl K';

function plainClick(event: React.MouseEvent) {
  return !(event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey);
}

export function Sidebar({
  collapsed,
  onToggleCollapsed,
  account,
  navigation,
  newAgent,
  onSearch,
  links = [],
  onHome,
  route = { kind: 'agent_manager' },
  onNavigate,
}: SidebarProps) {
  const mark = pixelBrand.mark[useBrandTheme()];
  const unavailableId = 'sidebar-new-agent-reason';
  const [settingsHost, setSettingsHost] = useState<HTMLDivElement | null>(null);
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
          <img className="sidebar__mark" src={mark} alt="" width={32} height={32} />
        </a>
        <ServiceMenu route={route} collapsed={collapsed} onNavigate={onNavigate ?? (() => {})} />
        {onToggleCollapsed === undefined ? null : (
          <button
            type="button"
            className="sidebar__toggle"
            aria-label={collapsed ? 'Show navigation' : 'Hide navigation'}
            aria-expanded={!collapsed}
            onClick={onToggleCollapsed}
          >
            <IconPanelLeftOutline16 />
          </button>
        )}
      </div>
      {newAgent === undefined && onSearch === undefined && links.length === 0 ? null : (
        <div className="sidebar__actions">
          {newAgent === undefined ? null : (
            <>
              <button
                type="button"
                className="sidebar__new"
                aria-current={newAgent.current ? 'page' : undefined}
                aria-label={newAgent.label ?? 'New agent'}
                title={collapsed ? (newAgent.label ?? 'New agent') : undefined}
                aria-disabled={newAgent.unavailableReason === undefined ? undefined : true}
                aria-describedby={
                  newAgent.unavailableReason === undefined ? undefined : unavailableId
                }
                onClick={() => {
                  if (newAgent.unavailableReason === undefined) newAgent.onCreate();
                }}
              >
                {newAgent.icon ?? <IconPlusOutline16 />}
                <span>{newAgent.label ?? 'New agent'}</span>
              </button>
              {newAgent.unavailableReason === undefined ? null : (
                <p id={unavailableId} className="sidebar__reason" hidden={collapsed}>
                  {newAgent.unavailableReason}
                </p>
              )}
            </>
          )}
          {onSearch === undefined ? null : (
            <button
              type="button"
              className="sidebar__search"
              aria-label="Search"
              aria-keyshortcuts="Meta+K Control+K"
              title={collapsed ? `Search (${SHORTCUT_HINT})` : undefined}
              onClick={onSearch}
            >
              <IconSearchOutline16 />
              <span>Search</span>
              <kbd className="sidebar__kbd">{SHORTCUT_HINT}</kbd>
            </button>
          )}
          {links.map((link) => (
            <a
              key={link.id}
              className="sidebar__link"
              href={routePath(link.route)}
              aria-current={link.current ? 'page' : undefined}
              title={collapsed ? link.label : undefined}
              onClick={(event) => {
                if (!plainClick(event) || onNavigate === undefined) return;
                event.preventDefault();
                onNavigate(link.route);
              }}
            >
              {link.icon}
              <span>{link.label}</span>
            </a>
          ))}
        </div>
      )}
      <div className="sidebar__navigation" hidden={collapsed}>
        <NavigationSettingsContext value={settingsHost}>{navigation}</NavigationSettingsContext>
      </div>
      <div className="sidebar__account">
        {account}
        <div className="sidebar__settings" ref={setSettingsHost} />
      </div>
    </aside>
  );
}
