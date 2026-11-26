import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useResourceActions } from './ResourceActions';
import { agentClient } from './test/agent';
import { sandboxFixture, parkedRuntimeUpdateFixture } from './test/hierarchy';
import type { CmaWebClient } from './api/client';
import { connectionClient, type ConnectionReport } from './connections/client';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function Actions({
  client,
  viewerId = 'owner',
  changed = vi.fn(),
}: {
  readonly client: CmaWebClient;
  readonly viewerId?: string;
  readonly changed?: (target: import('./agent/resourceChange').ResourceChange) => void;
}) {
  const actions = useResourceActions(client, changed, undefined, viewerId);
  return (
    <>
      {['first', 'second'].map((id) => (
        <button
          key={id}
          disabled={actions.pending || actions.busySandboxIds.has(id)}
          onClick={() => actions.select({ kind: 'pause_sandbox', sandboxId: id })}
        >
          Pause {id}
        </button>
      ))}
      <button
        disabled={actions.pending || actions.busySandboxIds.has('first')}
        onClick={() => actions.select({ kind: 'revive_sandbox', sandboxId: 'first' })}
      >
        Resume first
      </button>
      {actions.notice}
      {actions.error === undefined ? null : <p role="alert">{actions.error}</p>}
      {actions.overlay}
    </>
  );
}

it('observes concurrent accepted requests separately and releases each target only after confirmation', async () => {
  vi.useFakeTimers();
  const states = new Map([
    ['first', 'pausing'],
    ['second', 'pausing'],
  ]);
  const pauseSandbox = vi.fn().mockResolvedValue(undefined);
  const readSandbox = vi.fn(async (id: string) => ({
    ...sandboxFixture(states.get(id) === 'paused' ? 'paused' : 'pausing'),
    id,
  }));
  render(<Actions client={agentClient({ pauseSandbox, readSandbox })} />);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  expect(screen.getByRole('button', { name: 'Pause first' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Pause second' })).toBeEnabled();
  expect(screen.getByText('Pause request accepted.')).toBeVisible();
  expect(screen.getByText('Waiting for the Sandbox to pause.')).not.toBeVisible();
  expect(screen.queryByText('Sandbox paused.')).not.toBeInTheDocument();
  fireEvent.click(screen.getByText('Operation progress'));
  expect(screen.getByText('Waiting for the Sandbox to pause.')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss status' }));
  expect(screen.queryByRole('region', { name: 'Sandbox request' })).not.toBeInTheDocument();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause second' })));
  expect(pauseSandbox.mock.calls.map(([id]) => id)).toEqual(['first', 'second']);
  expect(screen.getByRole('button', { name: 'Pause second' })).toBeDisabled();
  states.set('first', 'paused');
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(screen.getByRole('button', { name: 'Pause first' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Pause second' })).toBeDisabled();
  expect(screen.queryByText('Sandbox paused.')).not.toBeInTheDocument();
  states.set('second', 'paused');
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(screen.getByRole('button', { name: 'Pause second' })).toBeEnabled();
  expect(screen.getByText('Sandbox paused.')).toBeInTheDocument();
  expect(pauseSandbox).toHaveBeenCalledTimes(2);
});

function readyConnections(owner = 'owner'): ConnectionReport {
  return {
    owner,
    ready: true,
    checked_at: '2026-10-15T00:00:00Z',
    services: [],
  };
}

it('checks resume connections silently and releases unrelated controls after acceptance', async () => {
  let finish!: (report: ConnectionReport) => void;
  const check = vi.spyOn(connectionClient, 'check').mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const reviveSandbox = vi.fn().mockResolvedValue(undefined);
  render(
    <Actions
      client={agentClient({
        reviveSandbox,
        readSandbox: async (id) => ({ ...sandboxFixture('reviving'), id }),
      })}
    />,
  );
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Resume first' })));
  expect(check).toHaveBeenCalledWith(
    { type: 'sandbox', id: 'first' },
    undefined,
    expect.any(AbortSignal),
  );
  expect(reviveSandbox).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await act(async () => finish(readyConnections()));
  expect(reviveSandbox).toHaveBeenCalledOnce();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByText('Resume request accepted.')).toBeVisible();
  expect(screen.getByText('Waiting for the Sandbox to resume.')).not.toBeVisible();
  expect(screen.getByRole('button', { name: 'Resume first' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Pause second' })).toBeEnabled();
  expect(screen.queryByText('Sandbox resumed.')).not.toBeInTheDocument();
});

it('opens connection assistance only for a missing required connection', async () => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: false,
    checked_at: '2026-10-15T00:00:00Z',
    services: [
      {
        service: 'github',
        display_name: 'GitHub',
        kind: 'nyxid',
        state: 'missing',
        connection_mode: 'hosted',
      },
    ],
  });
  const reviveSandbox = vi.fn().mockResolvedValue(undefined);
  render(<Actions client={agentClient({ reviveSandbox })} />);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Resume first' })));
  expect(screen.getByRole('dialog', { name: 'Connect your accounts' })).toBeVisible();
  expect(reviveSandbox).not.toHaveBeenCalled();
});

it.each(['unavailable', 'changed owner'] as const)(
  'keeps Resume held and exposes a failed connection check: %s',
  async (failure) => {
    const check = vi.spyOn(connectionClient, 'check');
    if (failure === 'unavailable') check.mockRejectedValue(new Error('Connections unavailable.'));
    else check.mockResolvedValue(readyConnections('another-owner'));
    const reviveSandbox = vi.fn();
    render(<Actions client={agentClient({ reviveSandbox })} />);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Resume first' })));
    expect(reviveSandbox).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent(
      failure === 'unavailable' ? 'Connections unavailable.' : 'Your CMA account changed.',
    );
    expect(screen.getByRole('button', { name: 'Resume first' })).toBeEnabled();
  },
);

it('cancels a pending connection check when the viewer changes', async () => {
  let finish!: (report: ConnectionReport) => void;
  const check = vi.spyOn(connectionClient, 'check').mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const reviveSandbox = vi.fn();
  const client = agentClient({ reviveSandbox });
  const view = render(<Actions client={client} />);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Resume first' })));
  view.rerender(<Actions client={client} viewerId="another-owner" />);
  expect(check.mock.calls[0]![2]?.aborted).toBe(true);
  await act(async () => finish(readyConnections()));
  expect(reviveSandbox).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('reports the latest action error and clears connection errors when another action succeeds', async () => {
  vi.spyOn(connectionClient, 'check').mockRejectedValue(new Error('Connections unavailable.'));
  const pauseSandbox = vi.fn().mockRejectedValueOnce(new Error('Pause outcome unknown.'));
  pauseSandbox.mockResolvedValueOnce(undefined);
  const reviveSandbox = vi.fn();
  render(
    <Actions
      client={agentClient({
        pauseSandbox,
        reviveSandbox,
        readSandbox: async (id) => ({ ...sandboxFixture('pausing'), id }),
      })}
    />,
  );
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  expect(screen.getByRole('alert')).toHaveTextContent('Pause outcome unknown.');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Resume first' })));
  expect(screen.getByRole('alert')).toHaveTextContent('Connections unavailable.');
  expect(reviveSandbox).not.toHaveBeenCalled();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(pauseSandbox.mock.calls[1]).toEqual(pauseSandbox.mock.calls[0]);
});

it('keeps an uncertain Pause visible and retains its key for explicit recovery', async () => {
  const pauseSandbox = vi.fn().mockRejectedValueOnce(new Error('Outcome unknown.'));
  pauseSandbox.mockResolvedValueOnce(undefined);
  render(
    <Actions
      client={agentClient({
        pauseSandbox,
        readSandbox: async (id) => ({ ...sandboxFixture('pausing'), id }),
      })}
    />,
  );
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  expect(screen.getByText(/Pause request is not confirmed/)).toBeVisible();
  expect(screen.queryByText('Operation progress')).not.toBeInTheDocument();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  expect(pauseSandbox).toHaveBeenCalledTimes(2);
  expect(pauseSandbox.mock.calls[1]).toEqual(pauseSandbox.mock.calls[0]);
  expect(screen.getByText('Pause request accepted.')).toBeVisible();
});

it('surfaces an eventual failure even after accepted status was dismissed', async () => {
  vi.useFakeTimers();
  let failed = false;
  const pauseSandbox = vi.fn().mockResolvedValue(undefined);
  render(
    <Actions
      client={agentClient({
        pauseSandbox,
        readSandbox: async (id) => ({
          ...sandboxFixture(failed ? 'failed' : 'pausing'),
          id,
        }),
      })}
    />,
  );
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss status' }));
  failed = true;
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(screen.getByText('Sandbox request needs attention.')).toBeVisible();
  expect(screen.getByRole('alert')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Pause first' })).toBeEnabled();
  expect(pauseSandbox).toHaveBeenCalledOnce();
});

it.each(['pause', 'resume'] as const)(
  'releases an accepted %s when a runtime update needs retry while still rotating',
  async (action) => {
    vi.useFakeTimers();
    vi.spyOn(connectionClient, 'check').mockResolvedValue(readyConnections());
    let failed = false;
    const command = vi.fn().mockResolvedValue(undefined);
    render(
      <Actions
        client={agentClient({
          pauseSandbox: command,
          reviveSandbox: command,
          readSandbox: async (id) => ({
            ...(failed
              ? parkedRuntimeUpdateFixture()
              : sandboxFixture(action === 'pause' ? 'pausing' : 'reviving')),
            id,
            runtimeUpdate: failed
              ? {
                  currentVersion: 'registry.example/runtime:old',
                  latestVersion: 'registry.example/runtime:new',
                  targetVersion: 'registry.example/runtime:new',
                  updateAvailable: true,
                  state: 'failed',
                  canUpdate: true,
                  schedule: { timezone: 'Asia/Singapore', localTime: '02:00' },
                  preservesHome: true,
                }
              : undefined,
          }),
        })}
      />,
    );
    const control = screen.getByRole('button', {
      name: action === 'pause' ? 'Pause first' : 'Resume first',
    });
    await act(async () => fireEvent.click(control));
    expect(control).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss status' }));
    failed = true;
    await act(() => vi.advanceTimersByTimeAsync(250));
    expect(control).toBeEnabled();
    expect(screen.getByText('Sandbox request needs attention.')).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent(
      'A runtime update needs attention. Open Sandbox details to retry it.',
    );
    expect(screen.queryByText(`Sandbox ${action === 'pause' ? 'paused' : 'resumed'}.`)).toBeNull();
    expect(command).toHaveBeenCalledOnce();
  },
);

it('surfaces failed background observations without unlocking or replaying the request', async () => {
  vi.useFakeTimers();
  const pauseSandbox = vi.fn().mockResolvedValue(undefined);
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce({ ...sandboxFixture('pausing'), id: 'first' })
    .mockRejectedValueOnce(new Error('Read unavailable.'))
    .mockResolvedValue({ ...sandboxFixture('pausing'), id: 'first' });
  render(<Actions client={agentClient({ pauseSandbox, readSandbox })} />);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss status' }));
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(screen.getByRole('alert')).toHaveTextContent('completion is not confirmed');
  expect(screen.getByRole('button', { name: 'Pause first' })).toBeDisabled();
  expect(screen.getByText('Waiting for the Sandbox to pause.')).not.toBeVisible();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.queryByRole('region', { name: 'Sandbox request' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause first' })).toBeDisabled();
  expect(pauseSandbox).toHaveBeenCalledOnce();
});

it('notifies the exact target on acceptance and completion, but not a failed command', async () => {
  vi.useFakeTimers();
  const changed = vi.fn();
  const pauseSandbox = vi
    .fn()
    .mockRejectedValueOnce(new Error('Not accepted'))
    .mockResolvedValue(undefined);
  let status: 'active' | 'paused' = 'active';
  render(
    <Actions
      changed={changed}
      client={agentClient({
        pauseSandbox,
        readSandbox: async (id) => ({ ...sandboxFixture(status), id }),
      })}
    />,
  );
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  expect(changed).not.toHaveBeenCalled();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Pause first' })));
  expect(changed).toHaveBeenCalledExactlyOnceWith({ kind: 'pause_sandbox', sandboxId: 'first' });
  status = 'paused';
  await act(() => vi.advanceTimersByTimeAsync(250));
  expect(changed.mock.calls).toEqual([
    [{ kind: 'pause_sandbox', sandboxId: 'first' }],
    [{ kind: 'pause_sandbox', sandboxId: 'first' }],
  ]);
});
