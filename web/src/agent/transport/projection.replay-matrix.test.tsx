import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../../api/context';
import { connectionClient } from '../../connections/client';
import { accessRequests } from '../accessRequests';
import { AgentSurface } from '../AgentSurface';
import { mergeTranscriptSnapshot } from '../chronology';
import { useAgentController } from '../controller';
import { conversationFromTranscript } from '../conversation';
import { v2AgentEventPage } from '../../api-client/agent-events-v2/types';
import { binding, cursor, harness, incarnation, item, json, page } from './controller.fixture';
import { foldV2Frame, foldV2Item } from './projection';

const agent = 'agt_1';
const reply = 'The requested file contains three entries.';
const toolTitle = 'Read the requested file';
const toolOutput = 'Synthetic file listing';
const history = [
  item(1, { type: 'turn_started', turn_sequence: 1 }, binding),
  item(2, { type: 'message', role: 'user', content: 'Read the file.' }),
  item(3, {
    type: 'tool_started',
    call_id: 'read',
    title: toolTitle,
    kind: 'read',
    status: 'running',
  }),
  item(4, {
    type: 'tool_updated',
    call_id: 'read',
    title: null,
    status: 'completed',
    output: { text: toolOutput, truncated: false },
  }),
  item(5, { type: 'status_changed', from: 'working', to: 'ready' }),
  item(6, { type: 'message', role: 'assistant', content: reply }),
  item(7, { type: 'turn_completed', turn_sequence: 1 }),
];
const beforeReply = 5;
const terminalSequence = history.length;
const reconnectAfter = [3, 5, 6];
const events = v2AgentEventPage(page(history, history.length), agent, 100).items;
const gap = {
  type: 'cma.agent.live_gap',
  data: { agent_id: agent, incarnation, turn_sequence: 1, reason: 'lagged', frames: 1 },
} as const;

function snapshot(through: number) {
  return events.slice(0, through).reduce(foldV2Item, {
    ...conversationFromTranscript({ agentId: agent, throughSeq: 0, turns: [] }),
    agentIncarnation: incarnation,
  });
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it.each(Array.from({ length: 1000 }, (_, seed) => seed))(
  'retains one command result across replay seed %i',
  (seed) => {
    let conversation = snapshot(0);
    for (const [index, event] of events.entries()) {
      conversation = foldV2Item(conversation, event);

      // The bits select overlap at each durable cursor and reconnect during the turn.
      if (seed & (1 << index)) {
        const replayed = events.slice(0, event.sequence).reduce(foldV2Item, conversation);
        expect(replayed).toEqual(conversation);
        conversation = replayed;
      }
      const reconnectIndex = reconnectAfter.indexOf(event.sequence);
      if (reconnectIndex >= 0 && seed & (1 << (events.length + reconnectIndex))) {
        conversation = foldV2Frame(conversation, gap, agent);
        conversation = mergeTranscriptSnapshot(conversation, snapshot(event.sequence));
      }
      if (event.sequence < terminalSequence) {
        expect(conversation.activeRunId).toBe('1');
        expect(conversation.turnOutcomes?.['1']).toBeUndefined();
        expect(conversation.messages.filter((message) => message.finalAnswer)).toHaveLength(0);
      }
    }

    const reloaded = snapshot(terminalSequence);
    for (const projected of [
      conversation,
      reloaded,
      mergeTranscriptSnapshot(conversation, reloaded),
    ]) {
      expect(projected.cursor).toBe(terminalSequence);
      expect(projected.activeRunId).toBeUndefined();
      expect(projected.turnOutcomes).toEqual({ '1': 'completed' });
      expect(projected.messages).toMatchObject([
        { role: 'user', commandId: binding.command_id },
        { role: 'assistant', content: reply, finalAnswer: true },
      ]);
      expect(projected.toolCalls).toMatchObject([
        { id: 'read', status: 'completed', output: { text: toolOutput } },
      ]);
      expect(projected.recovery).toEqual([]);
    }
  },
);

it('keeps the Agent recoverable through SSE reconnect and reload until it receives a terminal', async () => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: '2026-11-01T00:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  const transport = harness();
  const { client } = transport;

  function Agent() {
    const controller = useAgentController(client, agent, { viewerId: 'alice' });
    return (
      <CmaClientProvider client={client}>
        <AgentSurface
          agentId={agent}
          viewerId="alice"
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => undefined}
          onNavigate={() => undefined}
        />
      </CmaClientProvider>
    );
  }

  const subscriptions = () =>
    transport.fetch.mock.calls.filter(([url]) => String(url).includes('/events?'));
  const assertActive = () => {
    expect(screen.getByRole('textbox', { name: 'Message' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Stop response' })).toBeEnabled();
    expect(screen.getByText('Thinking', { exact: true })).toBeVisible();
    expect(screen.queryByText(reply)).not.toBeInTheDocument();
  };

  const view = render(<Agent />);
  await waitFor(() => expect(transport.streaming).toBe(true));
  await act(async () => {
    for (const event of history.slice(0, beforeReply))
      transport.durable(event.sequence, event.kind, event.response_binding);
    transport.live('reasoning_summary_delta', 'Checking the file', beforeReply);
  });
  assertActive();
  fireEvent.click(screen.getByRole('button', { name: /Read 1 file/ }));
  fireEvent.click(screen.getByRole('button', { name: new RegExp(toolTitle) }));
  expect(screen.getByLabelText('Recorded output')).toHaveTextContent(toolOutput);

  await act(async () => transport.end());
  await waitFor(() => expect(subscriptions()).toHaveLength(2));
  expect(
    new URL(String(subscriptions()[1]![0]), 'https://cma.example').searchParams.get('after_cursor'),
  ).toBe(cursor(beforeReply));
  await act(async () => transport.gap('reconnected', 1));
  expect(screen.getByText('Refreshing live updates…')).toBeVisible();
  assertActive();

  vi.useFakeTimers();
  vi.setSystemTime(new Date(history[0]!.appended_at));
  act(() => vi.advanceTimersByTime(60_000));
  assertActive();
  vi.useRealTimers();

  transport.snapshot.mockResolvedValueOnce(json(page(history.slice(0, beforeReply), beforeReply)));
  view.unmount();
  const restored = render(<Agent />);
  await waitFor(() => expect(subscriptions()).toHaveLength(3));
  assertActive();
  const [message, terminal] = history.slice(beforeReply);
  await act(async () => transport.durable(message!.sequence, message!.kind));
  expect(screen.getByRole('button', { name: 'Stop response' })).toBeEnabled();
  expect(screen.getByText('Thinking', { exact: true })).toBeVisible();
  await act(async () => transport.durable(terminal!.sequence, terminal!.kind));
  await waitFor(() =>
    expect(screen.queryByText('Thinking', { exact: true })).not.toBeInTheDocument(),
  );
  expect(screen.getAllByText(reply)).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBeEnabled();

  transport.snapshot.mockResolvedValueOnce(json(page(history, terminalSequence)));
  restored.unmount();
  render(<Agent />);
  await waitFor(() => expect(subscriptions()).toHaveLength(4));
  expect(screen.getAllByText(reply)).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Stop response' })).not.toBeInTheDocument();
  expect(transport.post).not.toHaveBeenCalled();
  expect(transport.client.runAgent).not.toHaveBeenCalled();
});
