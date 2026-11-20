import { render } from '@testing-library/react';
import { vi } from 'vitest';
import { navigationFixture } from '../test/hierarchy';
import type { NavigationSandbox } from './navigation';
import type { AppRoute } from '../routing/routes';
import { Tree, type TreeAction, type TreePermissions } from './Tree';

const FULL_ACCESS: TreePermissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};

export function renderTree(
  options: {
    readonly viewerId?: string;
    readonly view?: 'agent' | 'sandbox';
    readonly statusFilter?: string;
    readonly route?: AppRoute;
    readonly permissions?: TreePermissions;
    readonly paused?: boolean;
    readonly items?: readonly NavigationSandbox[];
    readonly refreshError?: string;
    readonly lifecycleBusy?: boolean;
    readonly busySandboxIds?: ReadonlySet<string>;
    readonly onAction?: (action: TreeAction) => void;
    readonly onNavigate?: (route: AppRoute) => void;
    readonly deletingSandboxIds?: ReadonlySet<string>;
    readonly onViewDeletion?: (sandboxId: string) => void;
  } = {},
) {
  return render(
    <Tree
      viewerId={options.viewerId ?? 'viewer-one'}
      view={options.view}
      statusFilter={options.statusFilter}
      items={options.items ?? navigationFixture(options.paused === true ? 'paused' : 'active')}
      refreshError={options.refreshError}
      lifecycleBusy={options.lifecycleBusy}
      busySandboxIds={options.busySandboxIds}
      deletingSandboxIds={options.deletingSandboxIds}
      onViewDeletion={options.onViewDeletion}
      route={options.route ?? { kind: 'home' }}
      permissions={options.permissions ?? FULL_ACCESS}
      onNavigate={options.onNavigate ?? vi.fn()}
      onAction={options.onAction ?? vi.fn()}
    />,
  );
}
