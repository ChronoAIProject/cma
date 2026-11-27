import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { AgentLiveEvent } from './api/types';
import { Console } from './Console';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { workspaceFixture } from './test/hierarchy';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('keeps header and selected sidebar status together across stream and navigation updates', async () => {
  vi.useFakeTimers();
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width: 1440,
    height: 900,
  } as DOMRect);
  let workspace = workspaceFixture();
  let update: ((event: AgentLiveEvent) => void) | undefined;
  const client = agentClient({
    readAgent: async () => AGENT_DETAIL,
    readWorkspace: async () => workspace,
    readAgentEvents: async () => ({
      items: [
        {
          seq: 1,
          appendedAt: AGENT_DETAIL.createdAt,
          kind: { type: 'status_changed', to: 'paused' },
        },
      ],
    }),
    replayAgent: async (_id, options) => {
      update = options.onEvent;
    },
  });
  render(
    <CmaClientProvider client={client}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  const header = () => document.querySelector('.agent-toolbar__status');
  const row = () => document.querySelector('.tree-row[data-node-id="agent:agt_1"]');
  expect(header()).toHaveTextContent('Ready');
  expect(row()).toHaveTextContent('Status unavailable');
  expect(update).toBeDefined();
  await act(async () =>
    update!({
      type: 'STATE_DELTA',
      threadId: 'agt_1',
      runId: 'run',
      timestamp: '2026-09-06T00:10:00Z',
      delta: [{ op: 'replace', path: '/status', value: 'working' }],
    }),
  );
  expect(header()).toHaveTextContent('Working');
  expect(row()).toHaveTextContent('Working');
  workspace = {
    ...workspace,
    agents: workspace.agents.map((agent) => ({
      ...agent,
      status: 'stopped',
      updatedAt: '2026-09-06T00:11:00Z',
    })),
  };
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(header()).toHaveTextContent('Agent stopped');
  expect(row()).toHaveTextContent('Stopped');
  expect(screen.getByText('The repository is ready.')).toBeInTheDocument();
});
import './test/connectedAccounts';
