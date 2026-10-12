import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import './Services.css';
export function ServiceTabs({
  route,
  onNavigate,
}: {
  readonly route: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const selected =
    route.kind === 'event_gateway' || route.kind === 'image_manager' ? route.kind : 'home';
  return (
    <nav className="service-tabs" aria-label="CMA services">
      {(
        [
          ['event_gateway', 'Event Gateway'],
          ['home', 'Agent Manager'],
          ['image_manager', 'Image Manager'],
        ] as const
      ).map(([kind, label]) => (
        <a
          key={kind}
          href={routePath({ kind })}
          aria-current={selected === kind ? 'page' : undefined}
          onClick={(event) => {
            if (
              event.button === 0 &&
              !event.metaKey &&
              !event.ctrlKey &&
              !event.shiftKey &&
              !event.altKey
            ) {
              event.preventDefault();
              onNavigate({ kind });
            }
          }}
        >
          {label}
        </a>
      ))}
    </nav>
  );
}
