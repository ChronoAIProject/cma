import type { AgentDetail, AgentStatus } from '../api/types';
import { routePath, type AppRoute } from '../routing/routes';
import { agentActions } from '../shell/treeActions';
import type { TreePermissions } from '../shell/Tree';
import type { MenuEntry } from '../ui/Menu';
import type { Crumb } from '../ui/PageHeader';

/**
 * The Agent header's More actions: Stop agent and Export history, then
 * Delete agent behind a separator — `docs/Web_Style.md` §11.
 *
 * The rows and their reasons are the sidebar's (`agentActions`), so the two
 * places an Agent can be stopped or deleted never disagree. Destructive rows
 * go last so a slip of the pointer down the list lands on something
 * reversible first.
 */
export function agentHeaderMenu(
  status: AgentStatus,
  permissions: TreePermissions | undefined,
  runtimeAvailable: boolean,
  busy: boolean,
): MenuEntry[] {
  if (permissions === undefined) return [];
  const rows = agentActions(status, permissions, runtimeAvailable).map(
    (action): MenuEntry & { readonly danger?: boolean } => ({
      id: action.id,
      label:
        action.id === 'stop_agent'
          ? 'Stop agent'
          : action.id === 'delete_agent'
            ? 'Delete agent'
            : action.label,
      icon: action.icon,
      danger: action.danger,
      disabled: action.disabled === true || busy,
      reason: busy ? 'Another action is still being confirmed.' : action.reason,
    }),
  );
  const safe = rows.filter((row) => row.danger !== true);
  const destructive = rows.filter((row) => row.danger === true);
  return safe.length > 0 && destructive.length > 0
    ? [...safe, { type: 'separator', id: 'before-destructive' }, ...destructive]
    : [...safe, ...destructive];
}

/** Where an owner's Agent lives, ending at the Agent itself. */
export function agentBreadcrumb(
  detail: Pick<AgentDetail, 'sandbox'>,
  title: string,
  onNavigate: (route: AppRoute) => void,
): Crumb[] {
  const sandbox = { kind: 'sandbox', sandboxId: detail.sandbox.id } as const;
  return [
    {
      label: detail.sandbox.name,
      href: routePath(sandbox),
      onSelect: () => onNavigate(sandbox),
    },
    { label: title },
  ];
}
