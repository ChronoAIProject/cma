import { vi } from 'vitest';
import type { AgentController, AgentResourceState } from './controller';
import type { ConversationState } from './conversation';

export function controller(
  state: AgentResourceState,
  conversation?: ConversationState,
  overrides: Partial<AgentController> = {},
): AgentController {
  return {
    inputAvailability: { allowed: true },
    resourceAvailability: { state: 'live', runtimeVersion: 'runtime-one' },
    state:
      state.status === 'ready' && conversation !== undefined ? { ...state, conversation } : state,
    reload: vi.fn(),
    clearActionError: vi.fn(),
    send: vi.fn().mockResolvedValue(undefined),
    interrupt: vi.fn().mockResolvedValue(undefined),
    approve: vi.fn().mockResolvedValue(undefined),
    stop: vi.fn().mockResolvedValue(undefined),
    delete: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  };
}
