import { vi } from 'vitest';
import type { AgentLifecycleOperationView, CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { taskExperience } from '../test/taskExperience';
import { lifecycleStorageKey } from './lifecycleIntent';

export const repositoryTask = {
  name: 'Repository research',
  initial_message: 'Read the repository and summarize its tests.',
  repository: { url: 'https://github.com/example/project', ref: 'main' },
};

export const repositoryFailure: AgentLifecycleOperationView = {
  id: 'operation-repository',
  agent_id: 'agt_1',
  status: 'failed',
  phase: 'failed',
  error: 'The repository could not be prepared.',
  events: [
    {
      sequence: 3,
      phase: 'workspace_runtime',
      status: 'failed',
      message: 'Repository preparation failed.',
      occurred_at: '2026-11-20T00:00:00Z',
    },
  ],
};

export function retainedRepositoryTask(owner = 'owner', bound = true) {
  const intent = {
    key: 'original-task-key',
    body: repositoryTask,
    receipt: { id: repositoryFailure.id },
    ...(bound ? { agentId: repositoryFailure.agent_id } : {}),
  };
  const key = lifecycleStorageKey(owner);
  const raw = JSON.stringify({ version: 1, intent });
  sessionStorage.setItem(key, raw);
  return { key, raw, intent };
}

export function repositoryClient(overrides: Partial<CmaWebClient> = {}) {
  return agentClient({
    taskExperience: taskExperience('owner'),
    readSandbox: async () => ({ ...sandboxFixture(), owner: 'owner' }),
    readTaskCreationSupport: async () => true,
    readTaskRepositorySupport: async () => true,
    createAgentLifecycle: vi.fn(),
    readAgentLifecycle: vi.fn(async () => repositoryFailure),
    ...overrides,
  });
}

export const confirmedConnections = {
  owner: 'owner',
  ready: true,
  checked_at: '2026-11-20T00:00:00Z',
  services: [],
};
