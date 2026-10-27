import type { AgentDetail, SandboxDetail } from '../api/types';
import { AGENT_DETAIL } from './agent';
import { sandboxFixture } from './hierarchy';

export const BOOTSTRAP_AGENT: AgentDetail = {
  ...AGENT_DETAIL,
  status: 'starting',
  readyAt: undefined,
  bootstrap: { operationId: 'operation-one', sandboxId: 'sbx_1', workspaceId: 'wsp_1' },
  workspace: { ...AGENT_DETAIL.workspace, status: 'preparing', resolvedCommit: undefined },
  worktree: { ...AGENT_DETAIL.worktree, status: 'creating', baseCommit: '' },
  sandbox: { ...AGENT_DETAIL.sandbox, status: 'provisioning' },
  firstMessage: { state: 'queued', queuedAt: AGENT_DETAIL.createdAt },
};

export function bootstrapSandbox(
  status: 'provisioning' | 'installing' = 'provisioning',
): SandboxDetail {
  return { ...sandboxFixture(status), owner: 'alice', runtimeVersion: undefined };
}
