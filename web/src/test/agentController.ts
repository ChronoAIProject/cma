import { vi } from 'vitest';
import { testClient } from './client';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE } from './agent';
import { sandboxFixture, workspaceFixture } from './hierarchy';

export function readableClient(overrides = {}) {
  return testClient({
    readAgent: vi.fn().mockResolvedValue(AGENT_DETAIL),
    readSandbox: vi.fn().mockResolvedValue(sandboxFixture()),
    readWorkspace: vi.fn().mockResolvedValue(workspaceFixture()),
    readAgentTranscript: vi.fn().mockResolvedValue(AGENT_TRANSCRIPT),
    readAgentUsage: vi.fn().mockResolvedValue(AGENT_USAGE),
    readAgentEvents: vi.fn().mockResolvedValue({ items: [] }),
    replayAgent: vi.fn(() => new Promise<void>(() => undefined)),
    ...overrides,
  });
}
