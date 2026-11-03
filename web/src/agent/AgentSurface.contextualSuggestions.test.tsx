import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { accessRequests } from './accessRequests';
import { AgentSurface } from './AgentSurface';
import { useAgentController } from './controller';
import { harness, item, json, page } from './transport/controller.fixture';

afterEach(() => vi.restoreAllMocks());

const saved = {
  status: 'ready',
  set: {
    id: 'saved-a',
    viewerId: 'alice',
    agentId: 'agt_1',
    answerId: 'event:3',
    scope: { turn_sequence: 1, answer_sequence: 3 },
    source: { kind: 'generated', version: 'contextual-v1' },
    items: [{ id: 'saved-a:0', text: 'Explain SQLite backup options.', rank: 0 }],
  },
};

async function setup(read: () => Promise<unknown>) {
  sessionStorage.clear();
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'alice',
    ready: true,
    checked_at: '2026-11-03T00:00:00Z',
    services: [],
  });
  vi.spyOn(accessRequests, 'list').mockResolvedValue([]);
  const fixture = harness();
  Object.assign(fixture.client, { readSuggestedReplies: read });
  fixture.snapshot.mockResolvedValue(
    json(
      page(
        [
          item(1, { type: 'turn_started', turn_sequence: 1 }),
          item(2, { type: 'message', role: 'user', content: 'Compare PostgreSQL and SQLite.' }),
          item(3, {
            type: 'message',
            role: 'assistant',
            content: 'SQLite suits this local-only app.',
          }),
          item(4, { type: 'turn_completed', turn_sequence: 1 }),
        ],
        4,
      ),
    ),
  );
  function Conversation({
    viewer = 'alice',
    accessUncertain = false,
  }: {
    viewer?: string;
    accessUncertain?: boolean;
  }) {
    const controller = useAgentController(fixture.client, 'agt_1', { viewerId: viewer });
    return (
      <CmaClientProvider client={fixture.client}>
        <AgentSurface
          agentId="agt_1"
          viewerId={viewer}
          accessUncertain={accessUncertain}
          lifecycleClient={fixture.client}
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={() => {}}
          onNavigate={() => {}}
        />
      </CmaClientProvider>
    );
  }
  const view = render(<Conversation />);
  await screen.findByText('SQLite suits this local-only app.');
  return {
    fixture,
    view,
    loseAccess: () => view.rerender(<Conversation accessUncertain />),
    changeViewer: () => view.rerender(<Conversation viewer="bob" />),
    reopen: () => {
      view.unmount();
      return render(<Conversation />);
    },
  };
}

it('never reopens answer A when its saved suggestions arrive after another message is accepted', async () => {
  let deliver: (value: unknown) => void = () => {};
  const read = vi.fn(
    () =>
      new Promise((resolve) => {
        deliver = resolve;
      }),
  );
  const { fixture } = await setup(read);
  await waitFor(() => expect(read).toHaveBeenCalled());
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Use SQLite.' } });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fixture.post).toHaveBeenCalledTimes(1));
  await act(async () => deliver(saved));
  expect(screen.queryByText('Explain SQLite backup options.')).not.toBeInTheDocument();
  expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  expect(composer).toBeEnabled();
});

it('shows a saved generated result after a pending read, reopens it, and recovers its exact conditional send', async () => {
  const read = vi
    .fn()
    .mockResolvedValueOnce({ status: 'pending', set: null })
    .mockResolvedValue(saved);
  const initial = await setup(read);
  expect(screen.queryByText('Suggested follow-ups')).not.toBeInTheDocument();
  const button = await screen.findByRole(
    'button',
    { name: saved.set.items[0]!.text },
    { timeout: 3000 },
  );
  expect(button).toBeEnabled();
  expect(read.mock.calls[0]?.slice(0, 2)).toEqual(['agt_1', saved.set.scope]);
  initial.view.unmount();
  const { fixture } = await setup(read);
  const restored = await screen.findByRole('button', { name: saved.set.items[0]!.text });
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Keep my existing notes.' } });
  fixture.post.mockRejectedValueOnce(new TypeError('Receipt lost'));
  fireEvent.click(restored);
  await screen.findByText('Awaiting confirmation');
  expect(restored).toBeDisabled();
  expect(composer).toHaveValue('Keep my existing notes.');
  fireEvent.click(screen.getByRole('button', { name: 'Recover pending request' }));
  await waitFor(() => expect(screen.queryByText('Suggested follow-ups')).not.toBeInTheDocument());
  const original = fixture.post.mock.calls[0]![0]!;
  expect(JSON.parse(String(original.body))).toEqual({
    input: saved.set.items[0]!.text,
    stream: true,
    answer_context: saved.set.scope,
  });
  expect(fixture.post.mock.calls[1]![0]!.body).toBe(original.body);
  expect(new Headers(fixture.post.mock.calls[1]![0]!.headers).get('Idempotency-Key')).toBe(
    new Headers(original.headers).get('Idempotency-Key'),
  );
  expect(composer).toHaveValue('Keep my existing notes.');
});

it.each(['empty', 'failed', 'unavailable', 'expired'])(
  'leaves the answer and composer usable for %s generation',
  async (status) => {
    await setup(async () => ({ status, set: null }));
    expect(screen.getByText('SQLite suits this local-only app.')).toBeVisible();
    expect(screen.getByRole('textbox', { name: 'Message' })).toBeEnabled();
    expect(screen.queryByRole('region', { name: 'Next you can ask' })).not.toBeInTheDocument();
  },
);

it.each(['account', 'permission'])('ignores a late result after %s changes', async (change) => {
  let deliver: (value: unknown) => void = () => {};
  const read = vi.fn(
    () =>
      new Promise((resolve) => {
        deliver = resolve;
      }),
  );
  const fixture = await setup(read);
  await waitFor(() => expect(read).toHaveBeenCalled());
  if (change === 'account') fixture.changeViewer();
  else fixture.loseAccess();
  await act(async () => deliver(saved));
  expect(screen.queryByText('Suggested follow-ups')).not.toBeInTheDocument();
});

it('counts visible exposure and confirmed send separately from the bound task outcome', async () => {
  const metrics: unknown[] = [];
  const listener = (event: Event) => metrics.push((event as CustomEvent).detail);
  window.addEventListener('cma:suggestion-metric', listener);
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      constructor(private readonly callback: (entries: unknown[]) => void) {}
      observe() {
        this.callback([{ isIntersecting: true, intersectionRatio: 1 }]);
      }
      disconnect() {}
    },
  );
  try {
    const { fixture } = await setup(async () => saved);
    const button = await screen.findByRole('button', { name: saved.set.items[0]!.text });
    await waitFor(() =>
      expect(metrics).toEqual([expect.objectContaining({ kind: 'exposure', setId: 'saved-a' })]),
    );
    fireEvent.click(button);
    await waitFor(() =>
      expect(metrics).toContainEqual(
        expect.objectContaining({ kind: 'send_confirmed', mode: 'original' }),
      ),
    );
    expect(metrics).not.toContainEqual(expect.objectContaining({ kind: 'task_outcome' }));
    await act(async () => {
      fixture.durable(
        5,
        { type: 'turn_started', turn_sequence: 5 },
        { response_id: 'resp_1', command_id: 'cmd_1', turn_sequence: 5 },
      );
      fixture.durable(
        6,
        { type: 'message', role: 'user', content: saved.set.items[0]!.text },
        null,
        'cmd_1',
      );
      fixture.durable(7, {
        type: 'turn_failed',
        turn_sequence: 5,
        failure: { class: 'runtime_error', detail: 'Task failed' },
      });
    });
    await waitFor(() =>
      expect(metrics, document.body.textContent ?? '').toContainEqual(
        expect.objectContaining({ kind: 'task_outcome', outcome: 'failed' }),
      ),
    );
  } finally {
    window.removeEventListener('cma:suggestion-metric', listener);
    vi.unstubAllGlobals();
  }
});
