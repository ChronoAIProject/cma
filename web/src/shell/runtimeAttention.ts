import { sandboxHasParkedRuntimeUpdate } from '../sandbox/parkedRuntimeUpdate';
import type { NavigationState } from './navigation';

export interface RuntimeAttentionSandbox {
  readonly sandboxId: string;
  readonly name: string;
  readonly agentIds: readonly string[];
}
export interface RuntimeAttention {
  readonly freshness: 'fresh' | 'stale' | 'unknown';
  readonly failedSandboxes: readonly RuntimeAttentionSandbox[];
}

export function runtimeAttention(
  navigation: NavigationState,
  deletion?: { readonly accepted: ReadonlySet<string>; readonly unknown: ReadonlySet<string> },
): RuntimeAttention {
  if (navigation.status !== 'ready') return { freshness: 'unknown', failedSandboxes: [] };
  let freshness: RuntimeAttention['freshness'] = navigation.refreshError ? 'stale' : 'fresh';
  const failed = new Map<string, RuntimeAttentionSandbox>();
  for (const { sandbox, workspaces } of navigation.items) {
    if (['retiring', 'retired'].includes(sandbox.status) || deletion?.accepted.has(sandbox.id))
      continue;
    if (deletion?.unknown.has(sandbox.id)) {
      freshness = 'stale';
      continue;
    }
    if (!sandboxHasParkedRuntimeUpdate(sandbox)) continue;
    const previous = failed.get(sandbox.id);
    failed.set(sandbox.id, {
      sandboxId: sandbox.id,
      name: sandbox.name,
      agentIds: [
        ...new Set([
          ...(previous?.agentIds ?? []),
          ...workspaces.flatMap((workspace) => workspace.agents.map((agent) => agent.id)),
        ]),
      ],
    });
  }
  return { freshness, failedSandboxes: [...failed.values()] };
}
