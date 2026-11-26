import type { CmaWebClient } from '../api/client';
import type { AppRoute } from '../routing/routes';
import type { TaskHomeOverview } from './TaskHomeLayout';

export interface TaskHomeProps {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
  readonly canCreateSandbox: boolean;
  readonly onCreated: (route: AppRoute) => void;
  readonly home?: TaskHomeOverview;
}
