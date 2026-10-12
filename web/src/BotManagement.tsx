import { lazy, Suspense, useState } from 'react';
import { useCmaClient, useCreationClient, useProfileViewsClient } from './api/context';
import type { User } from './auth/types';
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
export function isBotRoute(route: AppRoute): route is BotRoute {
  return ['bots', 'bot', 'bot_new'].includes(route.kind);
}
export function BotManagement({
  user,
  route,
  onNavigate,
}: {
  readonly user: User;
  readonly route: BotRoute;
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const client = useCmaClient();
  const profiles = useCreationClient();
  const profileViews = useProfileViewsClient();
  const [scope, setScope] = useState({ client, viewer: user.id, generation: 0 });
  if (scope.client !== client || scope.viewer !== user.id)
    setScope({ client, viewer: user.id, generation: scope.generation + 1 });
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
        viewerLabel={user.email ?? user.displayName ?? user.handle}
        profiles={profiles}
        agentProfiles={
          user.capabilities.includes('manage_own_profiles') ? profileViews?.agent : undefined
        }
        route={route}
        onNavigate={onNavigate}
      />
    </Suspense>
  );
}
