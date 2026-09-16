import { useState } from 'react';
import type { AppRoute } from '../routing/routes';
import type { BridgeViewsClient } from './client';
import { BridgeInventory } from './BridgeInventory';
import { BridgeDetail } from './BridgeDetail';
import { BridgeCreate } from './BridgeCreate';
import './Bridges.css';

export type BridgeRoute = Extract<AppRoute, { kind: 'bridges' | 'bridge' | 'bridge_new' }>;
export interface BridgePageProps {
  readonly client: BridgeViewsClient;
  readonly viewerId: string;
  readonly route: BridgeRoute;
  readonly onNavigate: (route: AppRoute) => void;
}

export function BridgePage(props: BridgePageProps) {
  const [scope, setScope] = useState({
    client: props.client,
    viewer: props.viewerId,
    generation: 0,
  });
  if (scope.client !== props.client || scope.viewer !== props.viewerId) {
    setScope({ client: props.client, viewer: props.viewerId, generation: scope.generation + 1 });
  }
  const route = props.route;
  const identity =
    route.kind === 'bridge' ? route.bridgeId : `${route.kind}:${route.organization ?? ''}`;
  return (
    <div className="bridges" key={`${scope.generation}:${identity}`}>
      {route.kind === 'bridge' ? (
        <BridgeDetail {...props} id={route.bridgeId} />
      ) : route.kind === 'bridge_new' ? (
        <BridgeCreate {...props} organization={route.organization} />
      ) : (
        <BridgeInventory {...props} organization={route.organization} />
      )}
    </div>
  );
}
