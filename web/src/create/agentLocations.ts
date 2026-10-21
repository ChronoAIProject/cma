import { useEffect, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxStatus, WorkspaceStatus } from '../api/types';
import { sandboxChildrenAvailable } from '../sandbox/lifecycle';
import { loadNavigation, type NavigationSandbox } from '../shell/navigation';

/** Where a new Agent works; each maps to one shape of the lifecycle request. */
export type AgentLocation =
  | { readonly kind: 'new_sandbox' }
  | { readonly kind: 'sandbox'; readonly sandboxId: string }
  | {
      readonly kind: 'workspace';
      readonly workspaceId: string;
      /** Kept when a caller names both, so its retained request keeps its slot. */
      readonly sandboxId?: string;
    };

export interface AgentLocationOption {
  readonly id: string;
  readonly location: AgentLocation;
  readonly title: string;
  /** The second line when the option can be used. */
  readonly detail?: string;
  readonly status?: SandboxStatus | WorkspaceStatus;
  /** Why the option cannot be used now. Listed and disabled, never removed. */
  readonly reason?: string;
}

export const SANDBOX_INACTIVE = 'Available when the Sandbox is active.';
export const WORKSPACE_NOT_READY = 'The Workspace is not ready.';
export const NO_SANDBOX_PERMISSION = 'You don’t have permission to create a Sandbox.';

export function locationId(location: AgentLocation): string {
  return location.kind === 'new_sandbox'
    ? 'new-sandbox'
    : location.kind === 'sandbox'
      ? `sandbox:${location.sandboxId}`
      : `workspace:${location.workspaceId}`;
}

export function locationScope(location: AgentLocation | undefined) {
  return {
    workspaceId: location?.kind === 'workspace' ? location.workspaceId : undefined,
    sandboxId:
      location?.kind === 'workspace' || location?.kind === 'sandbox'
        ? location.sandboxId
        : undefined,
  };
}

/**
 * Every place a new Agent could work, in the order the dialog lists them.
 *
 * `opened` is the target the dialog was opened for. It stays listed while the
 * hierarchy loads, fails to load or does not contain it, so the owner never
 * sees their chosen location disappear; its eligibility is then left to the
 * server rather than guessed.
 */
export function agentLocationOptions(
  items: readonly NavigationSandbox[],
  viewerId: string | undefined,
  canCreateSandbox: boolean,
  opened: AgentLocation | undefined,
  openedName?: string,
): AgentLocationOption[] {
  const owned = items.filter(
    ({ sandbox }) =>
      (sandbox.owner === undefined || sandbox.owner === viewerId) &&
      !['retiring', 'retired'].includes(sandbox.status),
  );
  const sandboxes = owned.map(({ sandbox }): AgentLocationOption => {
    const available = sandboxChildrenAvailable(sandbox);
    return {
      id: `sandbox:${sandbox.id}`,
      location: { kind: 'sandbox', sandboxId: sandbox.id },
      title: `New workspace in ${sandbox.name}`,
      status: sandbox.status,
      reason: available ? undefined : SANDBOX_INACTIVE,
    };
  });
  const workspaces = owned.flatMap(({ sandbox, workspaces }) =>
    workspaces
      .filter((workspace) => !['removing', 'removed'].includes(workspace.status))
      .map((workspace): AgentLocationOption => {
        const reason = !sandboxChildrenAvailable(sandbox)
          ? SANDBOX_INACTIVE
          : workspace.status !== 'ready'
            ? WORKSPACE_NOT_READY
            : undefined;
        return {
          id: `workspace:${workspace.id}`,
          location: { kind: 'workspace', workspaceId: workspace.id },
          title: `${sandbox.name} / ${workspace.name}`,
          status: workspace.status,
          reason,
        };
      }),
  );
  const options: AgentLocationOption[] = [
    {
      id: 'new-sandbox',
      location: { kind: 'new_sandbox' },
      title: 'New Sandbox',
      detail: 'Latest CMA runtime image',
      reason: canCreateSandbox ? undefined : NO_SANDBOX_PERMISSION,
    },
    ...sandboxes,
    ...workspaces,
  ];
  if (opened !== undefined && !options.some((option) => option.id === locationId(opened)))
    options.splice(1, 0, {
      id: locationId(opened),
      location: opened,
      title:
        opened.kind === 'workspace'
          ? (openedName ?? 'Selected workspace')
          : 'New workspace in this Sandbox',
    });
  return options;
}

export type AgentLocationsState =
  | { readonly status: 'loading' }
  | { readonly status: 'ready'; readonly items: readonly NavigationSandbox[] }
  | { readonly status: 'error' };

/**
 * The owner's hierarchy, read with the same public requests as the sidebar.
 *
 * Read once per open and on request: the dialog is short-lived, and the server
 * still refuses a location that changed after it was listed.
 */
export function useAgentLocations(client: CmaWebClient, enabled: boolean) {
  const [revision, setRevision] = useState(0);
  const [state, setState] = useState<AgentLocationsState>({ status: 'loading' });
  useEffect(() => {
    if (!enabled) return;
    const request = new AbortController();
    void loadNavigation(client, request.signal).then(
      (items) => {
        if (!request.signal.aborted) setState({ status: 'ready', items });
      },
      () => {
        // Surfaced in the dialog with a retry, as every other read failure is.
        if (!request.signal.aborted) setState({ status: 'error' });
      },
    );
    return () => request.abort();
  }, [client, enabled, revision]);
  return {
    state,
    retry: () => {
      setState({ status: 'loading' });
      setRevision((value) => value + 1);
    },
  };
}
