import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import type { SandboxEvent } from '../api/types';
import type { CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { activityDisplayEntries, activityEntries, activityEntry, activityLimit } from './activity';
import { LiveActivity } from './LiveActivity';
import { SandboxActivity } from './SandboxActivity';

const at = '2026-09-09T00:00:00Z';
const event = (seq: number, kind: SandboxEvent['kind'] = { type: 'created' }): SandboxEvent => ({
  sandboxId: 'sbx_1',
  seq,
  appendedAt: at,
  kind,
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('orders a display copy by real time then source and numeric sequence without changing ingestion', () => {
  const rows = [
    { id: 'sandbox-10', at, message: 'Sandbox tenth' },
    { id: 'sandbox-2', at, message: 'Sandbox second' },
    { id: 'agent-12', at, message: 'Agent twelfth' },
    { id: 'agent-3', at, message: 'Agent third' },
    { id: 'sandbox-20', at: '2026-09-08T23:59:59Z', message: 'Earlier timestamp' },
  ];
  const original = structuredClone(rows);
  render(<LiveActivity entries={rows} />);
  expect(
    screen.getAllByRole('listitem').map((row) => row.querySelector('span')?.textContent),
  ).toEqual([
    'Earlier timestamp',
    'Agent third',
    'Agent twelfth',
    'Sandbox second',
    'Sandbox tenth',
  ]);
  expect(rows).toEqual(original);
  expect(activityDisplayEntries(rows).map(({ id }) => id)).toEqual([
    'sandbox-20',
    'agent-3',
    'agent-12',
    'sandbox-2',
    'sandbox-10',
  ]);
});

it('keeps the existing retained window and equal-identity order when sorting display rows', () => {
  const rows = Array.from({ length: activityLimit + 1 }, (_, index) => ({
    id: `sandbox-${index + 1}`,
    at: new Date(Date.parse(at) - index * 1000).toISOString(),
    message: String(index),
  }));
  const displayed = activityDisplayEntries(rows);
  expect(displayed).toHaveLength(activityLimit);
  expect(displayed[0]?.id).toBe('sandbox-101');
  expect(displayed.at(-1)?.id).toBe('sandbox-2');
  expect(displayed.some(({ id }) => id === 'sandbox-1')).toBe(false);
  const equal = [
    { id: 'agent-2', at, message: 'First observation' },
    { id: 'agent-2', at, message: 'Second observation' },
  ];
  expect(activityDisplayEntries(equal)).toEqual(equal);
});

it('translates only lifecycle vocabulary, never payloads or unknown values', () => {
  const privateText = 'private diagnostic payload';
  expect(
    activityEntry(
      event(1, {
        type: 'work_step_failed',
        step: { kind: 'teardown_runtime' },
        failure: { detail: privateText },
      }),
      'sandbox',
    )?.message,
  ).toBe('Remove sandbox environment: needs attention');
  expect(
    activityEntry(event(2, { type: 'checkpoint_sealed', object_reference: privateText }), 'sandbox')
      ?.message,
  ).toBe('Progress saved');
  for (const value of [privateText, '__proto__', 'constructor']) {
    expect(
      activityEntry(event(3, { type: 'status_changed', to: value }), 'sandbox'),
    ).toBeUndefined();
    expect(
      activityEntry(event(3, { type: 'work_step_completed', step: { kind: value } }), 'sandbox'),
    ).toBeUndefined();
    expect(activityEntry(event(3, { type: value }), 'sandbox')).toBeUndefined();
  }
  for (const type of ['owner_message', 'agent_message', 'sign_in_required', 'tool_call_started'])
    expect(activityEntry(event(1, { type, content: privateText }), 'agent')).toBeUndefined();
  expect(activityEntry({ ...event(1), appendedAt: privateText }, 'sandbox')).toBeUndefined();
  expect(
    activityEntry(event(1, { type: 'work_step_completed', step: 'remove_home' }), 'agent')?.message,
  ).toBe('Remove agent files: completed');
});

it('distinguishes a scheduled provisioning retry from a terminal Sandbox failure', () => {
  const events = [
    event(1, {
      type: 'work_step_failed',
      step: { kind: 'provision_runtime' },
      failure: { class: 'retryable_transient', detail: 'private runtime diagnostic' },
    }),
    event(2, {
      type: 'failed',
      failure: { class: 'configuration', detail: 'private configuration diagnostic' },
    }),
  ];

  render(<LiveActivity entries={activityEntries(events, 'sandbox')} />);
  expect(screen.getByText('Start sandbox environment: retry scheduled')).toBeVisible();
  expect(screen.queryByText('Start sandbox environment: needs attention')).toBeNull();
  expect(screen.getByText('Sandbox needs attention')).toBeVisible();
  expect(screen.queryByText(/private .* diagnostic/)).toBeNull();
});

it('caps translated entries and display rows without retaining raw payloads', () => {
  const entries = activityEntries(
    Array.from({ length: 160 }, (_, n) => event(n + 1)),
    'sandbox',
  );
  expect(entries).toHaveLength(activityLimit);
  expect(entries[0]?.id).toBe('sandbox-61');
  expect(entries[0]).toEqual({
    id: 'sandbox-61',
    at: '2026-09-09T00:00:00.000Z',
    message: 'Sandbox created',
  });
  render(<LiveActivity entries={entries} syncedAt={at} />);
  expect(screen.getAllByRole('listitem')).toHaveLength(100);
  expect(screen.getByRole('region', { name: 'Recorded activity' })).toHaveAttribute(
    'tabindex',
    '0',
  );
  expect(screen.getByText(/Last synced/).querySelector('time')).toHaveAttribute('datetime', at);
});

it('preserves manual scrolling and follows new entries only at the bottom', () => {
  const first = activityEntries([event(1)], 'sandbox');
  const view = render(<LiveActivity entries={first} />);
  const node = screen.getByRole('region', { name: 'Recorded activity' });
  Object.defineProperties(node, {
    scrollHeight: { configurable: true, value: 1000 },
    clientHeight: { value: 180 },
  });
  node.scrollTop = 100;
  fireEvent.scroll(node);
  view.rerender(<LiveActivity entries={activityEntries([event(1), event(2)], 'sandbox')} />);
  expect(node.scrollTop).toBe(100);
  node.scrollTop = 820;
  fireEvent.scroll(node);
  view.rerender(
    <LiveActivity entries={activityEntries([event(1), event(2), event(3)], 'sandbox')} />,
  );
  expect(node.scrollTop).toBe(1000);
});

it('catches up in bounded batches, caps memory and advances sync time only when caught up', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(at);
  const readSandboxEvents = vi.fn(async (_id, options) =>
    options.cursor < 150 ? Array.from({ length: 30 }, (_, n) => event(options.cursor + n + 1)) : [],
  );
  render(<SandboxActivity client={agentClient({ readSandboxEvents })} sandboxId="sbx_1" />);
  await act(async () => {});
  expect(readSandboxEvents).toHaveBeenCalledTimes(4);
  expect(screen.getAllByRole('listitem')).toHaveLength(100);
  expect(screen.queryByText(/Last synced/)).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(readSandboxEvents.mock.calls.map((call) => call[1].cursor)).toEqual([
    0, 30, 60, 90, 120, 150,
  ]);
  expect(screen.getAllByRole('listitem')).toHaveLength(100);
  expect(screen.getByText(/Last synced/).querySelector('time')).toHaveAttribute(
    'datetime',
    '2026-09-09T00:00:01.500Z',
  );
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getAllByRole('listitem')).toHaveLength(100);
});

it('shows Sandbox Activity by event time, using sequence order for ties across pages', async () => {
  const readSandboxEvents = vi
    .fn<CmaWebClient['readSandboxEvents']>()
    .mockResolvedValueOnce([
      { ...event(1, { type: 'workspace_ready' }), appendedAt: '2026-09-09T00:00:02Z' },
      { ...event(2), appendedAt: '2026-09-09T00:00:01Z' },
    ])
    .mockResolvedValueOnce([
      { ...event(3, { type: 'workspace_created' }), appendedAt: '2026-09-09T08:00:01+08:00' },
      { ...event(4, { type: 'status_changed', to: 'active' }), appendedAt: '2026-09-09T00:00:03Z' },
    ])
    .mockResolvedValue([]);
  render(<SandboxActivity client={agentClient({ readSandboxEvents })} sandboxId="sbx_1" settled />);
  await screen.findByText('Sandbox: Live');

  const list = within(screen.getByRole('region', { name: 'Recorded activity' }));
  expect(
    list.getAllByRole('listitem').map((row) => row.querySelector('span')?.textContent),
  ).toEqual(['Sandbox created', 'Workspace created', 'Workspace ready', 'Sandbox: Live']);
  expect(readSandboxEvents.mock.calls.map(([, options]) => options?.cursor)).toEqual([0, 2, 4]);
});

it.each([401, 403, 404])(
  'clears activity after access denial %i and resumes from the start',
  async (status) => {
    vi.useFakeTimers();
    const readSandboxEvents = vi
      .fn()
      .mockResolvedValueOnce([event(1)])
      .mockResolvedValueOnce([])
      .mockRejectedValueOnce(new ApiError(status, 'private denial'))
      .mockResolvedValue([]);
    render(<SandboxActivity client={agentClient({ readSandboxEvents })} sandboxId="sbx_1" />);
    await act(async () => {});
    expect(screen.getByText('Sandbox created')).toBeInTheDocument();
    await act(() => vi.advanceTimersByTimeAsync(1500));
    expect(screen.queryByText('Sandbox created')).not.toBeInTheDocument();
    expect(screen.queryByText(/Last synced/)).not.toBeInTheDocument();
    expect(screen.queryByText('private denial')).not.toBeInTheDocument();
    await act(() => vi.advanceTimersByTimeAsync(5000));
    expect(readSandboxEvents.mock.calls.at(-1)?.[1].cursor).toBe(0);
  },
);

it('retains last activity and sync time on network failure, then resumes incrementally', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(at);
  const readSandboxEvents = vi
    .fn()
    .mockResolvedValueOnce([event(1)])
    .mockResolvedValueOnce([])
    .mockRejectedValueOnce(new Error('private network error'))
    .mockResolvedValue([]);
  render(<SandboxActivity client={agentClient({ readSandboxEvents })} sandboxId="sbx_1" />);
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByText('Sandbox created')).toBeInTheDocument();
  expect(screen.getByText(/Last synced/).querySelector('time')).toHaveAttribute(
    'datetime',
    '2026-09-09T00:00:00.000Z',
  );
  expect(screen.getByText('Activity updates interrupted. Reconnecting.')).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(readSandboxEvents.mock.calls.at(-1)?.[1].cursor).toBe(1);
  expect(screen.queryByText('Activity updates interrupted. Reconnecting.')).not.toBeInTheDocument();
});

it('does not commit a crossed or nonmonotonic page or display its contents', async () => {
  vi.useFakeTimers();
  const readSandboxEvents = vi
    .fn()
    .mockResolvedValueOnce([event(1), { ...event(2), sandboxId: 'sbx_other' }])
    .mockResolvedValueOnce([event(1)])
    .mockResolvedValueOnce([event(1)])
    .mockResolvedValue([]);
  render(<SandboxActivity client={agentClient({ readSandboxEvents })} sandboxId="sbx_1" />);
  await act(async () => {});
  expect(screen.queryByText('Sandbox created')).not.toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(readSandboxEvents.mock.calls.map((call) => call[1].cursor)).toEqual([0, 0, 1]);
  expect(screen.getAllByRole('listitem')).toHaveLength(1);
  expect(screen.queryByText(/Last synced/)).not.toBeInTheDocument();
});

it('avoids overlapping reads, suspends hidden-page polls, and aborts on unmount', async () => {
  vi.useFakeTimers();
  let resolve: (value: SandboxEvent[]) => void = () => {};
  const readSandboxEvents = vi.fn<CmaWebClient['readSandboxEvents']>(
    () =>
      new Promise<SandboxEvent[]>((done) => {
        resolve = done;
      }),
  );
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  const view = render(
    <SandboxActivity client={agentClient({ readSandboxEvents })} sandboxId="sbx_1" />,
  );
  await act(() => vi.advanceTimersByTimeAsync(10000));
  fireEvent(document, new Event('visibilitychange'));
  expect(readSandboxEvents).toHaveBeenCalledTimes(1);
  await act(async () => resolve([]));
  hidden.mockReturnValue(true);
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(readSandboxEvents).toHaveBeenCalledTimes(1);
  hidden.mockReturnValue(false);
  fireEvent(document, new Event('visibilitychange'));
  expect(readSandboxEvents).toHaveBeenCalledTimes(2);
  const signal = vi.mocked(readSandboxEvents).mock.calls.at(-1)?.[1]?.signal as AbortSignal;
  view.unmount();
  expect(signal.aborted).toBe(true);
  expect(vi.getTimerCount()).toBe(0);
});

it('shows a settled snapshot of activity once and clears the old sandbox on navigation', async () => {
  vi.useFakeTimers();
  const client = agentClient({
    readSandboxEvents: vi.fn(async (id, options) =>
      id === 'sbx_1' && options?.cursor === 0 ? [event(1)] : [],
    ),
  });
  const view = render(<SandboxActivity client={client} sandboxId="sbx_1" settled />);
  await act(async () => {});
  expect(screen.getByText('Sandbox created')).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(client.readSandboxEvents).toHaveBeenCalledTimes(2);
  view.rerender(<SandboxActivity client={client} sandboxId="sbx_2" settled />);
  expect(
    within(screen.getByRole('region', { name: 'Recorded activity' })).queryByText(
      'Sandbox created',
    ),
  ).not.toBeInTheDocument();
  await act(async () => {});
  expect(client.readSandboxEvents).toHaveBeenCalledTimes(3);
});
