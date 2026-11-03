import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { createRestWebClient } from '../api/rest';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { AgentSidePanel } from './AgentSidePanel';
import { AgentComputerObservation } from './environment/observation';
import { PlanCard } from './PlanCard';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript } from './conversation';
import type { ReactNode } from 'react';

afterEach(() => sessionStorage.clear());

const incarnation = 'a'.repeat(64);
const cursor = (sequence: number) => `ag1.${incarnation}.${sequence}`;

function fixture(subject = 'owner') {
  let historyStatus = 200;
  let environmentStatus = 200;
  let held = false;
  const waiting: (() => void)[] = [];
  const requests: string[] = [];
  const events: {
    sequence: number;
    cursor: string;
    appended_at: string;
    kind: object;
    response_binding: null;
  }[] = [];
  const append = (kind: object) => {
    const sequence = events.length + 1;
    events.push({
      sequence,
      cursor: cursor(sequence),
      appended_at: '2026-11-03T09:00:00Z',
      kind,
      response_binding: null,
    });
  };
  append({ type: 'turn_started', turn_sequence: 1 });
  const command = (number: number) => {
    append({
      type: 'tool_started',
      call_id: `command-${number}`,
      kind: 'execute',
      title: `printf operation-${number}`,
      status: 'in_progress',
    });
    append({
      type: 'tool_updated',
      call_id: `command-${number}`,
      title: null,
      status: 'completed',
      output: { text: `known-output-${number}`, truncated: false },
    });
  };
  const sdk = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input, init) => {
      const url = new URL(String(input));
      requests.push(`${init?.method ?? 'GET'} ${url.pathname}`);
      if (url.pathname.endsWith('/environment')) {
        if (environmentStatus !== 200)
          return Response.json(
            { code: 'forbidden', detail: 'Access changed' },
            { status: environmentStatus },
          );
        const feature = {
          state: 'available',
          reason: 'recorded_output_only',
          scope: 'agent_turn_history',
          writable: false,
        };
        const unsupported = { ...feature, state: 'unsupported' };
        return Response.json({
          agent_id: 'agt_1',
          policy_revision: 1,
          runtime_incarnation: null,
          revalidate_at: null,
          files: feature,
          operation_output: feature,
          replay: feature,
          browser: unsupported,
          control: unsupported,
        });
      }
      if (url.pathname.endsWith('/head'))
        return Response.json({
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_1',
          incarnation,
          through_cursor: cursor(events.length),
          active_turn: events[0],
        });
      if (url.pathname.endsWith('/snapshot')) {
        if (held) await new Promise<void>((resolve) => waiting.push(resolve));
        if (historyStatus !== 200)
          return Response.json(
            { code: 'agent_not_found', detail: 'Unavailable' },
            { status: historyStatus },
          );
        const parts = url.searchParams.get('page_cursor')?.split('.');
        const through = parts ? Number(parts[2]) : events.length;
        const after = parts ? Number(parts[3]) : 0;
        const items = events.slice(after, Math.min(through, after + 2));
        const delivered = after + items.length;
        return Response.json({
          object: 'cma.agent.event_page',
          schema_version: 1,
          agent_id: 'agt_1',
          incarnation,
          through_cursor: cursor(through),
          items,
          next_page_cursor:
            delivered < through ? `ags1.${incarnation}.${through}.${delivered}` : null,
        });
      }
      if (url.pathname.endsWith('/events')) {
        const after = Number(url.searchParams.get('after_cursor')?.split('.')[2] ?? 0);
        return new Response(
          events
            .slice(after)
            .map(
              (event) =>
                `event: cma.agent.event\nid: ${event.cursor}\ndata: ${JSON.stringify({ ...event, agent_id: 'agt_1', incarnation })}\n\n`,
            )
            .join(''),
          { headers: { 'content-type': 'text/event-stream' } },
        );
      }
      throw new Error(`Unexpected request ${url.pathname}`);
    },
  });
  const client = createRestWebClient(sdk);
  const observation = new AgentComputerObservation({ client, subject, agentId: 'agt_1' });
  const element = (extra?: ReactNode) => (
    <CmaClientProvider client={client}>
      {extra}
      <AgentSidePanel
        agentId="agt_1"
        shared={false}
        panel="activity"
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={new ArtifactPublicationScope()}
        observation={observation}
      />
    </CmaClientProvider>
  );
  const renderPanel = (extra?: ReactNode) => render(element(extra));
  return {
    command,
    append,
    requests,
    events,
    observation,
    renderPanel,
    element,
    holdHistory: () => {
      held = true;
    },
    releaseHistory: () => {
      held = false;
      waiting.splice(0).forEach((resolve) => resolve());
    },
    denyHistory: () => {
      historyStatus = 404;
    },
    expireHistory: () => {
      historyStatus = 410;
    },
    denyEnvironment: () => {
      environmentStatus = 403;
    },
  };
}

it('holds operation 2 when operation 4 arrives, then returns Live to 4 without executing', async () => {
  const scenario = fixture();
  scenario.command(1);
  scenario.command(2);
  scenario.command(3);
  scenario.renderPanel();
  expect(await screen.findByText('known-output-3')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Previous operation' }));
  expect(screen.getByText('known-output-2')).toBeVisible();
  await act(async () => {
    scenario.command(4);
    scenario.observation.publish([
      {
        id: 'command-4',
        runId: '1',
        kind: 'execute',
        title: 'printf operation-4',
        status: 'completed',
      },
    ]);
  });
  expect(screen.getByText('known-output-2')).toBeVisible();
  expect(screen.queryByText('known-output-4')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Return Live' }));
  expect(await screen.findByText('known-output-4')).toBeVisible();
  expect(screen.getByText(`Cursor ${cursor(9)}`)).toBeVisible();
  expect(
    scenario.events.filter((event) => 'type' in event.kind && event.kind.type === 'tool_started'),
  ).toHaveLength(4);
  expect(scenario.requests.every((request) => request.startsWith('GET '))).toBe(true);
});

it('reloads only the saved cursor from freshly authorized pages without future plan or output leakage', async () => {
  const scenario = fixture();
  scenario.append({
    type: 'plan_set',
    entries: [{ title: 'Original plan', status: 'in_progress' }],
  });
  scenario.command(1);
  scenario.command(2);
  const view = scenario.renderPanel();
  await screen.findByText('known-output-2');
  fireEvent.click(screen.getByRole('button', { name: 'Previous operation' }));
  expect(screen.getByText('known-output-1')).toBeVisible();
  view.unmount();
  scenario.append({ type: 'plan_set', entries: [{ title: 'Future plan', status: 'completed' }] });
  scenario.command(3);
  const reloaded = scenario.renderPanel();
  expect(await screen.findByText('known-output-1')).toBeVisible();
  expect(screen.getByText('Original plan: in_progress')).toBeVisible();
  expect(screen.queryByText(/Future plan/)).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('button', { name: 'Next operation' }), { key: 'ArrowRight' });
  expect(await screen.findByText('known-output-2')).toBeVisible();
  reloaded.unmount();
  scenario.denyHistory();
  scenario.renderPanel();
  expect(await screen.findByRole('alert')).toHaveTextContent('access is no longer available');
  expect(screen.queryByText('known-output-2')).not.toBeInTheDocument();
  expect(scenario.requests.every((request) => request.startsWith('GET '))).toBe(true);
});

it('shows explicit gaps and truncated output and never inherits a previous turn plan', async () => {
  const scenario = fixture();
  scenario.append({ type: 'plan_set', entries: [{ title: 'Old turn plan', status: 'completed' }] });
  scenario.command(1);
  scenario.append({ type: 'history_gap', frames: 2 });
  scenario.append({ type: 'turn_started', turn_sequence: 6 });
  scenario.command(2);
  scenario.append({
    type: 'tool_output_reported',
    call_id: 'command-2',
    output: { text: 'bounded output', truncated: true },
  });
  scenario.renderPanel();
  expect(await screen.findByText('bounded output')).toBeVisible();
  expect(screen.getByText('Output truncated')).toBeVisible();
  expect(screen.getByText(/History gap:/)).toBeVisible();
  expect(screen.getByText('Plan not recorded at this operation')).toBeVisible();
  expect(screen.queryByText(/Old turn plan/)).not.toBeInTheDocument();
  expect(screen.getByText('Turn 6')).toBeVisible();
});

it('keeps bounded pages navigable with keyboard controls across the 50-operation boundary', async () => {
  const scenario = fixture();
  for (let number = 1; number <= 52; number += 1) scenario.command(number);
  scenario.renderPanel();
  await screen.findByText('known-output-52');
  const previous = screen.getByRole('button', { name: 'Previous operation' });
  for (let count = 0; count < 49; count += 1) fireEvent.click(previous);
  expect(screen.getByText('known-output-3')).toBeVisible();
  fireEvent.keyDown(previous, { key: 'ArrowLeft' });
  expect(await screen.findByText('known-output-2')).toBeVisible();
  fireEvent.keyDown(previous, { key: 'End' });
  expect(await screen.findByText('known-output-52')).toBeVisible();
  expect(screen.getAllByRole('article')).toHaveLength(1);
});

it('opens the plan actual turn and returns Live to a newer turn without changing its identity', async () => {
  const scenario = fixture();
  scenario.command(1);
  scenario.command(2);
  scenario.append({ type: 'turn_completed', turn_sequence: 1 });
  scenario.append({ type: 'turn_started', turn_sequence: 7 });
  scenario.command(3);
  scenario.renderPanel(
    <PlanCard
      plan={{ id: 'plan:1', runId: '1', entries: [{ title: 'Old plan', status: 'completed' }] }}
      active={false}
      outcome="completed"
      onReplay={(turn) => scenario.observation.selectReplay(turn)}
    />,
  );
  await screen.findByText('known-output-3');
  fireEvent.click(screen.getByRole('button', { name: 'View plan and replay' }));
  expect(await screen.findByText('known-output-2')).toBeVisible();
  expect(screen.getByText('Turn 1')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Return Live' }));
  expect(await screen.findByText('known-output-3')).toBeVisible();
  expect(screen.getByText('Turn 7')).toBeVisible();
});

it('withdraws history when environment authority is revoked and keeps all historical controls unavailable', async () => {
  const scenario = fixture();
  scenario.command(1);
  scenario.command(2);
  scenario.renderPanel();
  await screen.findByText('known-output-2');
  fireEvent.click(screen.getByRole('button', { name: 'Previous operation' }));
  expect(screen.getByText('known-output-1')).toBeVisible();
  expect(screen.getByText(/History controls unavailable:/)).toBeVisible();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  scenario.denyEnvironment();
  fireEvent.focus(window);
  await waitFor(() => expect(screen.queryByText('known-output-1')).not.toBeInTheDocument());
  expect(screen.getByText(/Operation history access could not be checked/)).toBeVisible();
});

it('does not show another turn operations when the chosen completed turn has no commands', async () => {
  const scenario = fixture();
  scenario.command(1);
  scenario.append({ type: 'turn_completed', turn_sequence: 1 });
  scenario.append({ type: 'turn_started', turn_sequence: 5 });
  scenario.append({ type: 'turn_completed', turn_sequence: 5 });
  scenario.renderPanel(
    <PlanCard
      plan={{ id: 'plan:5', runId: '5', entries: [] }}
      active={false}
      outcome="completed"
      onReplay={(turn) => scenario.observation.selectReplay(turn)}
    />,
  );
  await screen.findByText('known-output-1');
  fireEvent.click(screen.getByRole('button', { name: 'View plan and replay' }));
  expect(await screen.findByText('No command operations recorded for this turn.')).toBeVisible();
  expect(screen.queryByText('known-output-1')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Return Live' }));
  expect(await screen.findByText('known-output-1')).toBeVisible();
});

it('advances Live with bounded unread public events instead of rescanning the historical prefix', async () => {
  const scenario = fixture();
  scenario.command(1);
  scenario.renderPanel();
  await screen.findByText('known-output-1');
  const snapshots = scenario.requests.filter((path) => path.endsWith('/snapshot')).length;
  await act(async () => {
    scenario.command(2);
    scenario.observation.publish([
      {
        id: 'command-2',
        runId: '1',
        kind: 'execute',
        title: 'printf operation-2',
        status: 'completed',
      },
    ]);
  });
  expect(await screen.findByText('known-output-2')).toBeVisible();
  expect(scenario.requests.filter((path) => path.endsWith('/snapshot'))).toHaveLength(snapshots);
  expect(scenario.requests).toContain('GET /api/v2/agents/agt_1/events');
  expect(screen.getByText(`Live cursor ${cursor(5)}`)).toBeVisible();
});

it('names expired history explicitly and does not silently replace the saved operation', async () => {
  const scenario = fixture();
  scenario.command(1);
  scenario.command(2);
  const view = scenario.renderPanel();
  await screen.findByText('known-output-2');
  fireEvent.click(screen.getByRole('button', { name: 'Previous operation' }));
  view.unmount();
  scenario.expireHistory();
  scenario.renderPanel();
  expect(await screen.findByRole('alert')).toHaveTextContent('Operation history expired.');
  expect(screen.queryByText('known-output-1')).not.toBeInTheDocument();
  expect(screen.queryByText('known-output-2')).not.toBeInTheDocument();
});

it('clears another account results and rejects its late authorized response', async () => {
  const original = fixture('first-owner');
  original.command(1);
  const view = original.renderPanel();
  await screen.findByText('known-output-1');
  original.holdHistory();
  fireEvent.click(screen.getByRole('button', { name: 'Return Live' }));
  const replacement = fixture('second-owner');
  replacement.command(2);
  view.rerender(replacement.element());
  expect(screen.queryByText('known-output-1')).not.toBeInTheDocument();
  await screen.findByText('known-output-2');
  await act(async () => original.releaseHistory());
  expect(screen.queryByText('known-output-1')).not.toBeInTheDocument();
  expect(screen.getByText('known-output-2')).toBeVisible();
});

it('opens a completed answer actual turn even when it reported no plan', async () => {
  const scenario = fixture();
  scenario.command(1);
  scenario.append({ type: 'turn_completed', turn_sequence: 1 });
  scenario.append({ type: 'turn_started', turn_sequence: 5 });
  scenario.command(2);
  const conversation = {
    ...conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
    messages: [
      {
        id: 'event:4',
        runId: '1',
        role: 'assistant' as const,
        content: 'First task finished',
        status: 'complete' as const,
        finalAnswer: true,
      },
    ],
  };
  scenario.renderPanel(
    <MessageStream
      conversation={conversation}
      onApproval={() => {}}
      onReplay={scenario.observation.selectReplay}
    />,
  );
  await screen.findByText('known-output-2');
  fireEvent.click(screen.getByRole('button', { name: 'View plan and replay' }));
  expect(await screen.findByText('known-output-1')).toBeVisible();
  expect(screen.getByText('Turn 1')).toBeVisible();
  expect(screen.getByText('Plan not recorded at this operation')).toBeVisible();
});
