import type { AgentDetail } from '../api/types';
import type { SelectedRetiredAgent } from '../useDeletionNoticeReconciliation';

export interface RetiredAgentIdentity {
  readonly viewerId: string;
  readonly authEpoch: number;
  readonly agentId: string;
  readonly worktreeId: string;
  readonly workspaceId: string;
  readonly sandboxId: string;
}
export interface RetiredAgentPresentation {
  readonly kind: 'retired';
  readonly identity: RetiredAgentIdentity;
  readonly detail: AgentDetail;
}

export function deriveRetiredPresentation(
  observed: SelectedRetiredAgent | undefined,
  current: {
    readonly viewerId?: string;
    readonly authEpoch?: number;
    readonly agentId?: string;
    readonly authorized: boolean;
  },
): RetiredAgentPresentation | { readonly kind: 'not-retired' } {
  const detail = observed?.detail;
  if (
    !current.authorized ||
    !current.viewerId ||
    current.authEpoch === undefined ||
    !current.agentId ||
    !observed ||
    observed.viewerId !== current.viewerId ||
    observed.authEpoch !== current.authEpoch ||
    !detail ||
    detail.id !== current.agentId ||
    detail.worktree.agentId !== detail.id ||
    detail.worktree.workspaceId !== detail.workspace.id ||
    detail.worktree.sandboxId !== detail.sandbox.id ||
    detail.sandbox.status !== 'retired'
  )
    return { kind: 'not-retired' };
  return {
    kind: 'retired',
    detail,
    identity: {
      viewerId: current.viewerId,
      authEpoch: current.authEpoch,
      agentId: detail.id,
      worktreeId: detail.worktree.id,
      workspaceId: detail.workspace.id,
      sandboxId: detail.sandbox.id,
    },
  };
}
