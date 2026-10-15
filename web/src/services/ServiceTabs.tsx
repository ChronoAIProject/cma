import type { AppRoute } from '../routing/routes';
import { routePath } from '../routing/routes';
import markMarkup from '../assets/brand/cma-mark.svg?raw';
import { BUILD } from '../shell/build';
import './Services.css';

const markHtml = { __html: markMarkup };

const SERVICE_COPY = {
  event_gateway: {
    title: 'Event Gateway',
    description: 'Route verified events into agents through durable channels.',
  },
  agent_manager: {
    title: 'Agent Manager',
    description: 'Create, configure and operate persistent agents.',
  },
  image_manager: {
    title: 'Image Manager',
    description: 'Build and share immutable runtime images.',
  },
} as const;
export function ServiceTabs({
  route,
  onNavigate,
}: {
  readonly route: AppRoute;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const selected =
    route.kind === 'event_gateway' || route.kind === 'image_manager' ? route.kind : 'agent_manager';
  return (
    <nav className="service-tabs" aria-label="CMA services">
      <a
        className="service-tabs__brand"
        href={routePath({ kind: 'home' })}
        aria-label="CMA home"
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
          onNavigate({ kind: 'home' });
        }}
      >
        <span
          className="service-tabs__mark"
          aria-hidden="true"
          dangerouslySetInnerHTML={markHtml}
        />
        <span>CMA</span>
      </a>
      <sub
        className="service-tabs__build"
        aria-label={`Build ${BUILD}`}
        title={`Build revision: ${BUILD}`}
      >
        {BUILD.replace(/^trureturing-/, '')}
      </sub>
      <span className="service-tabs__divider" aria-hidden="true" />
      {(
        [
          ['event_gateway', 'Event Gateway'],
          ['agent_manager', 'Agent Manager'],
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
          <span>{label}</span>
        </a>
      ))}
      <div className="service-tabs__description" aria-live="polite">
        <strong>{SERVICE_COPY[selected].title}</strong>
        <span>{SERVICE_COPY[selected].description}</span>
      </div>
    </nav>
  );
}
