import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import type { CmaWebClient } from '../api/client';
import type { AgentLiveEvent, AgentStatus } from '../api/types';
import { binding, harness } from './transport/controller.fixture';

afterEach(() => sessionStorage.clear());

function Agent({ client }: { readonly client: CmaWebClient }) {
  const controller = useAgentController(client, 'agt_1');
  return (
    <CmaClientProvider client={client}>
      <AgentSurface
        agentId="agt_1"
        controller={controller}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />
    </CmaClientProvider>
  );
}

it('shows confirmed elapsed time in the header from recorded start and successful end', async () => {
  const client = agentClient({
    readAgentTranscript: async () => ({
      ...AGENT_TRANSCRIPT,
      turns: [
        {
          ...AGENT_TRANSCRIPT.turns[0]!,
          startedAt: '2026-11-03T10:00:00Z',
          outcome: { type: 'completed', at: '2026-11-03T10:04:00Z' },
        },
      ],
    }),
  });
  render(<Agent client={client} />);
  const completed = await screen.findByText('Done in 4m');
  expect(within(completed.closest('header')!).getByRole('status')).toHaveTextContent('Done in 4m');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBeEnabled();
});

it('shows completion only after the durable successful Responses turn and clears it for the next turn', async () => {
  const transport = harness();
  render(<Agent client={transport.client} />);
  await waitFor(() => expect(transport.streaming).toBe(true));
  await act(async () => transport.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding));
  expect(screen.queryByText(/Done in/)).not.toBeInTheDocument();
  await act(async () => transport.durable(2, { type: 'turn_completed', turn_sequence: 1 }));
  expect(await screen.findByText('Done in <1s')).toBeVisible();
  await act(async () => transport.durable(3, { type: 'turn_started', turn_sequence: 3 }));
  expect(screen.queryByText(/Done in/)).not.toBeInTheDocument();
  await act(async () => transport.durable(4, { type: 'turn_cancelled', turn_sequence: 3 }));
  expect(screen.queryByText(/Done in/)).not.toBeInTheDocument();
  expect(transport.client.runAgent).not.toHaveBeenCalled();
});

it('keeps a recorded output gap failed in completion and plan displays without failing the next turn', async () => {
  const transport = harness();
  render(<Agent client={transport.client} />);
  await waitFor(() => expect(transport.streaming).toBe(true));
  await act(async () => {
    transport.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
    transport.durable(2, {
      type: 'plan_set',
      entries: [{ title: 'Report result', status: 'completed' }],
    });
    transport.durable(3, {
      type: 'message',
      role: 'assistant',
      content: 'Retained partial answer',
    });
    transport.durable(4, { type: 'history_gap', frames: 2 });
    transport.durable(5, { type: 'turn_completed', turn_sequence: 1 });
  });
  expect(screen.queryByText(/Done in/)).not.toBeInTheDocument();
  expect(screen.getByText('Response failed')).toBeVisible();
  expect(screen.getByText('Retained partial answer')).toBeVisible();
  expect(screen.getByText('Incomplete conversation history')).toBeVisible();
  await act(async () => {
    transport.durable(6, { type: 'turn_started', turn_sequence: 6 });
    transport.durable(7, { type: 'message', role: 'assistant', content: 'Complete next answer' });
    transport.durable(8, { type: 'turn_completed', turn_sequence: 6 });
  });
  expect(await screen.findByText('Done in <1s')).toBeVisible();
  expect(screen.getByText('Response failed')).toBeVisible();
  expect(transport.client.runAgent).not.toHaveBeenCalled();
});

it('replaces the previous completion with a newly confirmed AG-UI duration without remounting the draft', async () => {
  let receive: ((event: AgentLiveEvent) => void) | undefined;
  const client = agentClient({
    readAgentTranscript: async () => ({
      ...AGENT_TRANSCRIPT,
      turns: [
        {
          ...AGENT_TRANSCRIPT.turns[0]!,
          outcome: { type: 'completed', at: '2026-09-06T00:05:00Z' },
        },
      ],
    }),
    replayAgent: async (_agent, options) => {
      receive = options.onEvent;
      await new Promise<void>((resolve) =>
        options.signal?.addEventListener('abort', () => resolve()),
      );
    },
  });
  render(<Agent client={client} />);
  await screen.findByText('Done in 4m');
  await waitFor(() => expect(receive).toBeDefined());
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Keep typing while the header changes' } });
  act(() =>
    receive!({
      type: 'RUN_STARTED',
      threadId: 'agt_1',
      runId: '5',
      timestamp: '2026-11-03T10:00:00Z',
      cursor: 5,
    }),
  );
  expect(screen.queryByText(/Done in/)).not.toBeInTheDocument();
  act(() =>
    receive!({
      type: 'RUN_FINISHED',
      threadId: 'agt_1',
      runId: '5',
      timestamp: '2026-11-03T10:00:00.300Z',
      outcome: { type: 'complete' },
      cursor: 6,
    }),
  );
  expect(await screen.findByText('Done in <1s')).toBeVisible();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(field);
  expect(field).toHaveValue('Keep typing while the header changes');
});

it.each(['stopped', 'failed', 'deleted'] as const)(
  'withholds an earlier completion when the Agent is %s',
  async (status: AgentStatus) => {
    const client = agentClient({
      readAgent: async () => ({ ...AGENT_DETAIL, status }),
      readAgentTranscript: async () => ({
        ...AGENT_TRANSCRIPT,
        turns: [
          {
            ...AGENT_TRANSCRIPT.turns[0]!,
            outcome: { type: 'completed', at: '2026-09-06T00:05:00Z' },
          },
        ],
      }),
    });
    render(<Agent client={client} />);
    await screen.findByText('The repository is ready.');
    expect(screen.queryByText(/Done in/)).not.toBeInTheDocument();
  },
);

it.each([
  {
    name: 'missing start',
    startedAt: '',
    outcome: { type: 'completed', at: '2026-11-03T10:04:00Z' },
  },
  { name: 'missing end', startedAt: '2026-11-03T10:00:00Z', outcome: { type: 'completed' } },
  {
    name: 'invalid start',
    startedAt: 'unknown',
    outcome: { type: 'completed', at: '2026-11-03T10:04:00Z' },
  },
  {
    name: 'invalid end',
    startedAt: '2026-11-03T10:00:00Z',
    outcome: { type: 'completed', at: 'unknown' },
  },
  {
    name: 'reversed timestamps',
    startedAt: '2026-11-03T10:05:00Z',
    outcome: { type: 'completed', at: '2026-11-03T10:04:00Z' },
  },
  {
    name: 'failure',
    startedAt: '2026-11-03T10:00:00Z',
    outcome: { type: 'failed', at: '2026-11-03T10:04:00Z' },
  },
  {
    name: 'interruption',
    startedAt: '2026-11-03T10:00:00Z',
    outcome: { type: 'interrupted', at: '2026-11-03T10:04:00Z' },
  },
  { name: 'active turn', startedAt: '2026-11-03T10:00:00Z', outcome: { type: 'active' } },
])(
  'omits completion for the latest history with $name instead of reusing an older success',
  async ({ startedAt, outcome }) => {
    const client = agentClient({
      readAgentTranscript: async () => ({
        ...AGENT_TRANSCRIPT,
        throughSeq: 8,
        turns: [
          { ...AGENT_TRANSCRIPT.turns[0]!, turnSequence: 5, messages: [], startedAt, outcome },
          {
            ...AGENT_TRANSCRIPT.turns[0]!,
            outcome: { type: 'completed', at: '2026-09-06T00:05:00Z' },
          },
        ],
      }),
    });
    render(<Agent client={client} />);
    await screen.findByText('The repository is ready.');
    expect(screen.queryByText(/Done in/)).not.toBeInTheDocument();
  },
);

it.each([
  { name: 'stop', event: { type: 'RUN_FINISHED', outcome: { type: 'interrupt', interrupts: [] } } },
  { name: 'failure', event: { type: 'RUN_ERROR', message: 'Provider failed', code: 'provider' } },
  {
    name: 'approval pause',
    event: {
      type: 'RUN_FINISHED',
      outcome: { type: 'interrupt', interrupts: [{ id: 'approval-one', reason: 'confirmation' }] },
    },
  },
  { name: 'unknown outcome', event: { type: 'RUN_FINISHED' } },
])('never calls an AG-UI $name successful completion', async ({ event }) => {
  let receive: ((event: AgentLiveEvent) => void) | undefined;
  const client = agentClient({
    replayAgent: async (_agent, options) => {
      receive = options.onEvent;
      await new Promise<void>((resolve) =>
        options.signal?.addEventListener('abort', () => resolve()),
      );
    },
  });
  render(<Agent client={client} />);
  await waitFor(() => expect(receive).toBeDefined());
  act(() =>
    receive!({
      type: 'RUN_STARTED',
      threadId: 'agt_1',
      runId: '5',
      timestamp: '2026-11-03T10:00:00Z',
      cursor: 5,
    }),
  );
  act(() =>
    receive!({
      ...event,
      threadId: 'agt_1',
      runId: '5',
      timestamp: '2026-11-03T10:04:00Z',
      cursor: 6,
    }),
  );
  expect(screen.queryByText(/Done in/)).not.toBeInTheDocument();
});
