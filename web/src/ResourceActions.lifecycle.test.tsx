import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useResourceActions } from './ResourceActions';
import { agentClient } from './test/agent';
import { sandboxFixture } from './test/hierarchy';
import type { CmaWebClient } from './api/client';
import { connectionClient } from './connections/client';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function Actions({ client }: { readonly client: CmaWebClient }) {
  const actions = useResourceActions(client, vi.fn(), undefined, 'owner');
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
  fireEvent.click(screen.getByRole('button', { name: 'Hide progress' }));
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

it('releases unrelated controls after a connection-checked resume is accepted', async () => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-10-15T00:00:00Z',
    services: [],
  });
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
  expect(reviveSandbox).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Resume first' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Pause second' })).toBeEnabled();
  expect(screen.queryByText('Sandbox resumed.')).not.toBeInTheDocument();
});
