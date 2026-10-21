import { lazy, Suspense } from 'react';
import type { AppRoute } from '../routing/routes';

const EventGatewayDocs = lazy(() =>
  import('./EventGatewayDocs').then((module) => ({ default: module.EventGatewayDocs })),
);
const DocsIndex = lazy(() =>
  import('./EventGatewayDocs').then((module) => ({ default: module.DocsIndex })),
);

/** Documentation routes share their loading boundary and lazy module. */
export function DocsPage({
  view,
  onNavigate,
}: {
  readonly view: Extract<AppRoute, { kind: 'docs' }>['view'];
  readonly onNavigate: (route: AppRoute) => void;
}) {
  return (
    <Suspense fallback={<p role="status">Loading documentation</p>}>
      {view === 'event_gateway_protocol' ? (
        <EventGatewayDocs onNavigate={onNavigate} />
      ) : (
        <DocsIndex onNavigate={onNavigate} />
      )}
    </Suspense>
  );
}
