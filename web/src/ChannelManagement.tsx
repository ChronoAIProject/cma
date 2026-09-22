import { lazy, Suspense, useState } from 'react';
import { useCmaClient, useCreationClient } from './api/context';
import type { User } from './auth/types';
import { BridgeEntry } from './bridges/BridgeEntry';
import type { BridgeRoute } from './bridges/BridgePage';
import type { BotRoute } from './bots/BotPage';
import type { AppRoute } from './routing/routes';
import type { CmaWebClient } from './api/client';

export function canManageBots(user: User, client: CmaWebClient): boolean {
  return (
    !user.visitor && user.capabilities.includes('manage_own_bots') && client.botViews !== undefined
  );
}

const BotPage = lazy(() =>
  import('./bots/BotPage').then((module) => ({ default: module.BotPage })),
);
export function isChannelRoute(route: AppRoute): route is BridgeRoute | BotRoute {
  return ['bridges', 'bridge', 'bridge_new', 'bots', 'bot', 'bot_new'].includes(route.kind);
}
export function ChannelManagement({
  user,
  route,
  onNavigate,
}: {
  readonly user: User;
  readonly route: BridgeRoute | BotRoute;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const client = useCmaClient();
  const profiles = useCreationClient();
  const [scope, setScope] = useState({ client, viewer: user.id, generation: 0 });
  if (scope.client !== client || scope.viewer !== user.id)
    setScope({ client, viewer: user.id, generation: scope.generation + 1 });
  if (route.kind === 'bridges' || route.kind === 'bridge' || route.kind === 'bridge_new')
    return (
      <BridgeEntry
        client={user.visitor ? undefined : client.bridgeViews}
        viewerId={user.id}
        route={route}
        onNavigate={onNavigate}
      />
    );
  if (
    user.visitor ||
    !user.capabilities.includes('manage_own_bots') ||
    client.botViews === undefined
  )
    return (
      <section className="console__empty">
        <h1>Bot management unavailable</h1>
        <p>Your account needs permission to manage its Bots.</p>
      </section>
    );
  return (
    <Suspense
      fallback={
        <section className="console__empty" role="status">
          Loading Bots
        </section>
      }
    >
      <BotPage
        key={scope.generation}
        client={client.botViews}
        viewer={user.id}
        profiles={profiles}
        route={route}
        onNavigate={onNavigate}
      />
    </Suspense>
  );
}
