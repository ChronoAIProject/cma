import type { AgentDetail, AgentTranscript, AgentUsage } from '../api/types';
import type { CmaWebClient } from '../api/client';
import { testClient } from './client';
import { sandboxFixture, workspaceFixture } from './hierarchy';

export function agentClient(overrides: Partial<CmaWebClient> = {}): CmaWebClient {
  return testClient({
    listSandboxes: async () => ({ items: [sandboxFixture()] }),
    readSandbox: async () => sandboxFixture(),
    readWorkspace: async () => workspaceFixture(),
    readAgent: async () => AGENT_DETAIL,
    readAgentTranscript: async () => AGENT_TRANSCRIPT,
    readAgentUsage: async () => AGENT_USAGE,
    readAgentEvents: async () => ({ items: [] }),
    readSandboxEvents: async () => [],
    replayAgent: async (_id, options) =>
      new Promise<void>((resolve) => {
        options.signal?.addEventListener('abort', () => resolve(), { once: true });
      }),
    ...overrides,
  });
}

export const AGENT_DETAIL: AgentDetail = {
  id: 'agt_1',
  status: 'ready',
  profile: {
    id: 'agp_default',
    revision: 4,
    digest: 'agent-digest',
    model: 'gpt-5',
    thinkingEffort: 'medium',
    permissionLevel: 'workspace_write',
    skills: [{ source: 'ornn', id: 'typescript' }],
  },
  worktree: {
    id: 'wkt_1',
    workspaceId: 'wsp_1',
    sandboxId: 'sbx_1',
    name: 'agent-one',
    path: '/worktrees/agent-one',
    branch: 'agent/one',
    baseCommit: 'abc123',
    status: 'ready',
    agentId: 'agt_1',
  },
  workspace: {
    id: 'wsp_1',
    name: 'chrono-bot',
    path: '/workspaces/chrono-bot',
    status: 'ready',
    profile: { id: 'wkp_default', revision: 2, digest: 'workspace-digest' },
    resolvedCommit: 'abc123',
  },
  sandbox: {
    id: 'sbx_1',
    name: 'Atlas',
    status: 'active',
    profile: { id: 'sbp_default', revision: 1, digest: 'sandbox-digest' },
  },
  conformsTo: ['https://spec.chrono-ai.fun/cma/1.0/conf/conversation'],
  reach: ['github'],
  title: 'Agent one',
  firstMessage: {
    state: 'completed',
    queuedAt: '2026-09-06T00:00:00Z',
    completedAt: '2026-09-06T00:01:00Z',
  },
  createdAt: '2026-09-06T00:00:00Z',
  updatedAt: '2026-09-06T00:05:00Z',
  readyAt: '2026-09-06T00:00:30Z',
};

export const AGENT_TRANSCRIPT: AgentTranscript = {
  agentId: 'agt_1',
  throughSeq: 4,
  turns: [
    {
      turnSequence: 1,
      startedAt: '2026-09-06T00:01:00Z',
      ownerMessage: 'Inspect this repository.',
      agentMessages: ['The repository is ready.'],
      messages: [
        {
          sequence: 2,
          role: 'user',
          content: 'Inspect this repository.',
          appendedAt: '2026-09-06T00:01:00Z',
        },
        {
          sequence: 3,
          role: 'assistant',
          content: 'The repository is ready.',
          appendedAt: '2026-09-06T00:01:01Z',
        },
      ],
      outcome: { type: 'completed' },
    },
  ],
};

export const AGENT_USAGE: AgentUsage = {
  agentId: 'agt_1',
  throughSeq: 4,
  inputTokens: 120,
  outputTokens: 60,
  contextUsed: 400,
  contextWindow: 1000,
};
