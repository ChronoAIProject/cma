import type { CmaWebClient } from './api/client';
import type { User } from './auth/types';
import type { AppRoute } from './routing/routes';
import { TaskHome } from './create/TaskHome';
import { WelcomeView } from './ConsoleViews';

export function ConsoleLanding({
  client,
  user,
  route,
  canCreate,
  onCreated,
}: {
  readonly client: CmaWebClient;
  readonly user: User;
  readonly route: AppRoute;
  readonly canCreate: boolean;
  readonly onCreated: (route: AppRoute) => void;
}) {
  const home = route.kind === 'home' || route.kind === 'agent_manager';
  return home && canCreate ? (
    <TaskHome
      client={client}
      viewerId={user.id}
      canCreateSandbox={user.capabilities.includes('create_sandbox')}
      onCreated={onCreated}
    />
  ) : (
    <WelcomeView heading={home ? 'Welcome to CMA' : route.kind.replaceAll('_', ' ')} />
  );
}
