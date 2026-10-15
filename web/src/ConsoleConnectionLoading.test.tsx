import { act, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient, ApiError } from '@chronoai/cma-sdk';
import { Console } from './Console';
import { CmaClientProvider } from './api/context';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { WIRE_AGENT } from './test/rest';
import { connectionClient } from './connections/client';

afterEach(() => vi.restoreAllMocks());

it.each(['personal', 'refused'] as const)(
  'starts loading connections only after exact personal access resolves: %s',
  async (access) => {
    const sdk = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async () => Response.json({ ...WIRE_AGENT, id: 'agt_1' }),
    });
    const authorized = await sdk.agents.readAccessible('agt_1');
    let finish!: (value: typeof authorized) => void;
    let refuse!: (error: Error) => void;
    vi.spyOn(sdk.agents, 'readAccessible').mockReturnValue(
      new Promise((resolve, reject) => {
        finish = resolve;
        refuse = reject;
      }),
    );
    const detail = vi.fn(() => new Promise<typeof AGENT_DETAIL>(() => {}));
    const check = vi.spyOn(connectionClient, 'check').mockResolvedValue({
      owner: 'owner',
      ready: true,
      checked_at: '2026-10-14T00:00:00Z',
      services: [],
    });
    const client = agentClient({ resourceViews: sdk, readAgent: detail });
    const parent = vi.spyOn(client, 'readSandbox');
    const view = render(
      <CmaClientProvider client={client}>
        <Console
          user={{
            id: 'owner',
            handle: 'Owner',
            visitor: false,
            capabilities: ['use_own_sandboxes'],
          }}
          route={{ kind: 'agent', agentId: 'agt_1' }}
          onSignOut={vi.fn()}
          onNavigate={vi.fn()}
        />
      </CmaClientProvider>,
    );
    await waitFor(() => expect(sdk.agents.readAccessible).toHaveBeenCalledOnce());
    expect(check).not.toHaveBeenCalled();
    expect(detail).not.toHaveBeenCalled();
    expect(parent).not.toHaveBeenCalledWith(WIRE_AGENT.sandbox.id, expect.anything());
    await act(async () => {
      if (access === 'personal') finish(authorized);
      else refuse(new ApiError(403, 'Not allowed'));
    });
    if (access === 'personal') {
      await waitFor(() => expect(check).toHaveBeenCalledOnce());
      expect(detail).toHaveBeenCalledOnce();
      expect(parent.mock.calls.filter(([id]) => id === WIRE_AGENT.sandbox.id)).toHaveLength(1);
      expect(parent).toHaveBeenCalledWith(WIRE_AGENT.sandbox.id, {
        signal: expect.any(AbortSignal),
      });
      expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
    } else {
      expect(screen.getByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
      expect(check).not.toHaveBeenCalled();
      expect(detail).not.toHaveBeenCalled();
      expect(parent).not.toHaveBeenCalledWith(WIRE_AGENT.sandbox.id, expect.anything());
    }
    view.unmount();
  },
);
