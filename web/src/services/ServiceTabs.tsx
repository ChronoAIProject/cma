import { isBotRoute } from '../BotManagement';
import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import { IconAgentPresetOutline16, IconCodeOutline16, IconBrowseOutline16 } from '../ui/icons';
import './Services.css';
import { useServiceIndicator } from './ServiceSwitchMotion';

export function ServiceTabs({
  route,
  onNavigate,
  collapsed = false,
}: {
  readonly route: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
  readonly collapsed?: boolean;
}) {
  const gateway = route.kind === 'event_gateway' || isBotRoute(route);
  const indicator = useServiceIndicator(collapsed);
  return (
    <nav
      className="service-tabs"
      aria-label="CMA services"
      data-collapsed={collapsed || undefined}
      data-selected={gateway ? 'gateway' : 'agent'}
    >
      <span ref={indicator} className="service-tabs__indicator" aria-hidden="true" />
      {(
        [
          ['agent_manager', 'Agent Manager', IconAgentPresetOutline16],
          ['event_gateway', 'Event Gateway', IconCodeOutline16],
        ] as const
      ).map(([kind, label, Icon]) => (
        <a
          key={kind}
          href={routePath({ kind })}
          title={label}
          aria-label={label}
          aria-current={
            (gateway ? kind === 'event_gateway' : kind === 'agent_manager') ? 'page' : undefined
          }
          onClick={(event) => {
            if (
              event.button !== 0 ||
              event.metaKey ||
              event.ctrlKey ||
              event.shiftKey ||
              event.altKey
            )
              return;
            event.preventDefault();
            onNavigate({ kind });
          }}
        >
          <Icon />
          <span>{label}</span>
        </a>
      ))}
      <button
        type="button"
        disabled
        aria-label="Image Manager (coming soon)"
        title="Image Manager — coming soon"
      >
        <IconBrowseOutline16 />
        <span>Image Manager</span>
      </button>
    </nav>
  );
}
