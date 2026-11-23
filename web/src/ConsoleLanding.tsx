import type { CmaWebClient } from './api/client';
import type { User } from './auth/types';
import type { AppRoute } from './routing/routes';
import { TaskHome } from './create/TaskHome';
import { WelcomeView } from './ConsoleViews';
import type { TaskHomeOverview } from './create/TaskHomeLayout';

export function ConsoleLanding({
  client,
  user,
  route,
  canCreate,
  onCreated,
  home,
}: {
  readonly client: CmaWebClient;
  readonly user: User;
  readonly route: AppRoute;
  readonly canCreate: boolean;
  readonly onCreated: (route: AppRoute) => void;
  readonly home?: TaskHomeOverview;
}) {
  const isHome = route.kind === 'home' || route.kind === 'agent_manager';
  return isHome && canCreate ? (
    <TaskHome
      client={client}
      viewerId={user.id}
      canCreateSandbox={user.capabilities.includes('create_sandbox')}
      onCreated={onCreated}
      home={home}
    />
  ) : (
    <WelcomeView heading={isHome ? 'Welcome to CMA' : route.kind.replaceAll('_', ' ')} />
  );
}
