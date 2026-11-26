import { useState } from 'react';
import { isBotRoute } from '../BotManagement';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { Menu, type MenuEntry } from '../ui/Menu';
import { IconAgentPresetOutline16, IconChevronDownOutline14, IconCodeOutline16 } from '../ui/icons';
import './Services.css';

type Service = 'agent_manager' | 'event_gateway';

const SERVICE_LABEL: Readonly<Record<Service, string>> = {
  agent_manager: 'Agent Manager',
  event_gateway: 'Event Gateway',
};

/** Which service a route belongs to. Consumers live under Event Gateway. */
export function serviceFor(route: AppRoute): Service {
  return route.kind === 'event_gateway' || isBotRoute(route) ? 'event_gateway' : 'agent_manager';
}

/**
 * The one service switch — the shared style contracts
 *
 * The menu keeps seldom-used service switching from occupying a sidebar row.
 */
export function ServiceMenu({
  route,
  onNavigate,
  collapsed = false,
}: {
  readonly route: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
  readonly collapsed?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const current = serviceFor(route);
  const items: MenuEntry[] = [
    {
      id: 'agent_manager',
      label: SERVICE_LABEL.agent_manager,
      icon: <IconAgentPresetOutline16 />,
      href: routePath({ kind: 'agent_manager' }),
    },
    {
      id: 'event_gateway',
      label: SERVICE_LABEL.event_gateway,
      icon: <IconCodeOutline16 />,
      href: routePath({ kind: 'event_gateway' }),
    },
  ];
  return (
    <Menu
      open={open}
      label="CMA services"
      placement={`service-menu${collapsed ? ' service-menu--collapsed' : ''}`}
      matchAnchorWidth={!collapsed}
      items={items}
      selectedId={current}
      onClose={() => setOpen(false)}
      onSelect={(id) => {
        setOpen(false);
        if (id === 'agent_manager' || id === 'event_gateway') onNavigate({ kind: id });
      }}
      anchor={
        <button
          type="button"
          className="service-menu__trigger"
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label={`Service: ${SERVICE_LABEL[current]}`}
          title={collapsed ? SERVICE_LABEL[current] : undefined}
          onClick={() => setOpen((wasOpen) => !wasOpen)}
        >
          <span className="service-menu__icon" aria-hidden="true">
            {current === 'event_gateway' ? <IconCodeOutline16 /> : <IconAgentPresetOutline16 />}
          </span>
          <span className="service-menu__name">{SERVICE_LABEL[current]}</span>
          <IconChevronDownOutline14 className="service-menu__chevron" />
        </button>
      }
    />
  );
}
