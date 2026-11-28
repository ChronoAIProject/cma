import type { AppRoute } from '../routing/routes';
import { agentDisplayTitle } from '../agent/presentation';
import { sandboxChildStatus, sandboxLifecycleLabel } from '../sandbox/lifecycle';
import { statusLabel } from '../vocabulary/status';
import { APPEARANCE_ORDER, appearanceLabel, type Appearance } from '../theme/appearance';
import {
  IconAgentPresetOutline16,
  IconBranchOutline16,
  IconDarkOutline16,
  IconDataOutline16,
  IconFolderOpenOutline16,
  IconFollowsystemOutline16,
  IconLightOutline16,
  IconPlusOutline16,
  IconRightUpOutline16,
} from '../ui/icons';
import { taskActivityOrder } from './taskGroups';
import type { Command } from './CommandMenu';
import type { NavigationSandbox } from './navigation';

export interface ShellCommandOptions {
  readonly items: readonly NavigationSandbox[];
  readonly onNavigate: (route: AppRoute) => void;
  readonly onOpenEnvironments: () => void;
  /** Present only when New agent can be used right now. */
  readonly onNewAgent?: () => void;
  readonly canOpenTriggers: boolean;
  readonly canOpenOperations: boolean;
  readonly onAppearance: (appearance: Appearance) => void;
}

/**
 * Everything the command menu offers, built from the navigation already read.
 *
 * Deleted and retired resources are left out: the menu is for going somewhere
 * that exists, and the sidebar still shows deletion progress.
 */
export function shellCommands({
  items,
  onNavigate,
  onOpenEnvironments,
  onNewAgent,
  canOpenTriggers,
  canOpenOperations,
  onAppearance,
}: ShellCommandOptions): Command[] {
  const agents: { readonly command: Command; readonly at: number }[] = [];
  const sandboxes: Command[] = [];
  const workspaces: Command[] = [];
  for (const { sandbox, workspaces: children } of items) {
    if (sandbox.status === 'retired' || sandbox.status === 'retiring') continue;
    sandboxes.push({
      id: `sandbox:${sandbox.id}`,
      searchTerms: [sandbox.id],
      group: 'Sandboxes',
      label: sandbox.name,
      meta: sandbox.id,
      hint: sandboxLifecycleLabel(sandbox),
      icon: <IconDataOutline16 />,
      run: () => onNavigate({ kind: 'sandbox', sandboxId: sandbox.id }),
    });
    for (const workspace of children) {
      if (workspace.status === 'removed') continue;
      workspaces.push({
        id: `workspace:${workspace.id}`,
        searchTerms: [workspace.id],
        group: 'Workspaces',
        label: workspace.name,
        meta: sandbox.name,
        hint: statusLabel(sandboxChildStatus(sandbox, workspace.status)),
        icon:
          workspace.source.type === 'git_repository' ? (
            <IconBranchOutline16 />
          ) : (
            <IconFolderOpenOutline16 />
          ),
        run: () => onNavigate({ kind: 'workspace', workspaceId: workspace.id }),
      });
      for (const agent of workspace.agents) {
        if (agent.status === 'deleted') continue;
        agents.push({
          at: taskActivityOrder(agent),
          command: {
            id: `agent:${agent.id}`,
            searchTerms: [agent.id],
            group: 'Agents',
            label: agentDisplayTitle(agent),
            meta: `${workspace.name} · ${sandbox.name}`,
            hint: statusLabel(sandboxChildStatus(sandbox, agent.status)),
            icon: <IconAgentPresetOutline16 />,
            run: () => onNavigate({ kind: 'agent', agentId: agent.id }),
          },
        });
      }
    }
  }
  const go = (id: string, label: string, route: AppRoute): Command => ({
    id: `go:${id}`,
    group: 'Go to',
    label,
    icon: <IconRightUpOutline16 />,
    run: () => onNavigate(route),
  });
  const actions: Command[] = [
    ...(onNewAgent === undefined
      ? []
      : [
          {
            id: 'action:new-agent',
            group: 'Actions',
            label: 'New agent',
            icon: <IconPlusOutline16 />,
            run: onNewAgent,
          },
        ]),
    ...APPEARANCE_ORDER.map((appearance) => ({
      id: `action:appearance-${appearance}`,
      group: 'Actions',
      label: `Appearance: ${appearanceLabel(appearance)}`,
      icon:
        appearance === 'light' ? (
          <IconLightOutline16 />
        ) : appearance === 'dark' ? (
          <IconDarkOutline16 />
        ) : (
          <IconFollowsystemOutline16 />
        ),
      run: () => onAppearance(appearance),
    })),
  ];
  const destinations: Command[] = [
    go('agent-manager', 'Agent Manager', { kind: 'agent_manager' }),
    go('event-gateway', 'Event Gateway', { kind: 'event_gateway' }),
    {
      id: 'go:environments',
      group: 'Go to',
      label: 'Environments',
      preferredQueries: ['environment', 'environments'],
      icon: <IconRightUpOutline16 />,
      run: onOpenEnvironments,
    },
    ...(canOpenTriggers ? [go('triggers', 'Triggers', { kind: 'triggers' })] : []),
    go('artifacts', 'Artifacts', { kind: 'artifacts' }),
    go('docs', 'Docs', { kind: 'docs' }),
    ...(canOpenOperations ? [go('operations', 'Operations', { kind: 'operations' })] : []),
  ];
  // Most recent first, so an empty query already lists the likely target.
  const recent = [...agents]
    .sort((a, b) => b.at - a.at || a.command.id.localeCompare(b.command.id))
    .map(({ command }) => command);
  return [...recent, ...sandboxes, ...workspaces, ...actions, ...destinations];
}
