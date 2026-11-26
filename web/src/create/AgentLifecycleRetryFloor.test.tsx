import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { createRestWebClient } from '../api/rest';
import { agentClient } from '../test/agent';
import { AgentLifecycleCard } from './AgentLifecycleCard';
import { lifecycleStorageKey } from './lifecycleIntent';

const saved = JSON.stringify({
  version: 1,
  intent: {
    key: 'retained-key',
    body: { name: 'Retained agent' },
    receipt: { id: 'original-operation' },
  },
});
const ready = {
  id: 'original-operation',
  status: 'succeeded',
  phase: 'agent_startup',
  agent_id: 'original-agent',
  events: [],
};
beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  sessionStorage.setItem(lifecycleStorageKey('owner'), saved);
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
async function advance(ms: number) {
  await act(async () => vi.advanceTimersByTimeAsync(ms));
}
function fixture(retryable: boolean) {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(
      new Response(
        JSON.stringify({
          status: 503,
          code: 'store_failed',
          retryable,
          detail: 'private upstream detail',
        }),
        { status: 503, headers: { 'Retry-After': '3' } },
      ),
    )
    .mockImplementation(async () => new Response(JSON.stringify(ready)));
  vi.stubGlobal('fetch', fetcher);
  const rest = createRestWebClient(createClient({ baseUrl: 'https://cma.test' }));
  const create = vi.fn();
  const opened = vi.fn();
  const client = agentClient({
    readAgentLifecycle: rest.readAgentLifecycle,
    createAgentLifecycle: create,
  });
  const props = { client, viewerId: 'owner', onClose: vi.fn(), onCreated: opened };
  return { fetcher, create, opened, props };
}

it('stops explicit non-retryable lifecycle failures while retaining the command', async () => {
  const f = fixture(false);
  const view = render(<AgentLifecycleCard {...f.props} />);
  await advance(60_000);
  expect(f.fetcher).toHaveBeenCalledOnce();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(saved);
  expect(f.create).not.toHaveBeenCalled();
  expect(f.opened).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Retry status' })).toBeEnabled();
  expect(screen.queryByText(/private upstream/)).not.toBeInTheDocument();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('honors the floor across manual retries and remount then opens the original Agent once', async () => {
  const f = fixture(true);
  const first = render(<AgentLifecycleCard {...f.props} />);
  await advance(0);
  fireEvent.click(screen.getByRole('button', { name: 'Retry status' }));
  await advance(1000);
  expect(f.fetcher).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button', { name: 'Retry status' }));
  first.unmount();
  const second = render(<AgentLifecycleCard {...f.props} />);
  await advance(1999);
  expect(f.fetcher).toHaveBeenCalledOnce();
  await advance(1);
  expect(f.fetcher).toHaveBeenCalledTimes(2);
  expect(f.opened).toHaveBeenCalledExactlyOnceWith({ id: 'original-agent', status: 'ready' });
  expect(f.create).not.toHaveBeenCalled();
  second.unmount();
  await advance(0);
  expect(vi.getTimerCount()).toBe(0);
});

it('paces the existing automatic lifecycle retry at exactly the server floor', async () => {
  const f = fixture(true);
  const view = render(<AgentLifecycleCard {...f.props} />);
  await advance(2999);
  expect(f.fetcher).toHaveBeenCalledOnce();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(saved);
  await advance(1);
  expect(f.fetcher).toHaveBeenCalledTimes(2);
  expect(f.opened).toHaveBeenCalledExactlyOnceWith({ id: 'original-agent', status: 'ready' });
  expect(f.create).not.toHaveBeenCalled();
  view.unmount();
  await advance(0);
  expect(vi.getTimerCount()).toBe(0);
});

it('allows an explicit fresh status read after a false refusal without replaying creation', async () => {
  const f = fixture(false);
  const view = render(<AgentLifecycleCard {...f.props} />);
  await advance(0);
  fireEvent.click(screen.getByRole('button', { name: 'Retry status' }));
  await advance(2999);
  expect(f.fetcher).toHaveBeenCalledOnce();
  await advance(1);
  expect(f.fetcher).toHaveBeenCalledTimes(2);
  expect(f.opened).toHaveBeenCalledOnce();
  expect(f.create).not.toHaveBeenCalled();
  view.unmount();
  await advance(0);
  expect(vi.getTimerCount()).toBe(0);
});

it.each([
  '{"retryable":"false","code":"busy"}',
  '{"status":200,"retryable":true,"code":"busy"}',
  '<html>private upstream</html>',
])('stops malformed lifecycle failure metadata: %s', async (body) => {
  const f = fixture(true);
  f.fetcher
    .mockReset()
    .mockImplementation(
      async () => new Response(body, { status: 503, headers: { 'Retry-After': '3' } }),
    );
  const view = render(<AgentLifecycleCard {...f.props} />);
  await advance(60_000);
  expect(f.fetcher).toHaveBeenCalledOnce();
  expect(f.opened).not.toHaveBeenCalled();
  expect(f.create).not.toHaveBeenCalled();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(saved);
  expect(screen.queryByText(/private upstream/)).toBeNull();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('does not cap a long server floor to the five-second failure backoff', async () => {
  const f = fixture(true);
  f.fetcher
    .mockReset()
    .mockResolvedValueOnce(
      new Response('{"code":"busy","retryable":true}', {
        status: 429,
        headers: { 'Retry-After': '12' },
      }),
    )
    .mockImplementation(async () => new Response(JSON.stringify(ready)));
  const view = render(<AgentLifecycleCard {...f.props} />);
  await advance(11999);
  expect(f.fetcher).toHaveBeenCalledOnce();
  await advance(1);
  expect(f.fetcher).toHaveBeenCalledTimes(2);
  expect(f.opened).toHaveBeenCalledOnce();
  expect(f.create).not.toHaveBeenCalled();
  view.unmount();
  await advance(0);
  expect(vi.getTimerCount()).toBe(0);
});

it('cancels an old client waiter without publishing or dispatching it after replacement', async () => {
  const f = fixture(true);
  const view = render(<AgentLifecycleCard {...f.props} />);
  await advance(500);
  const replacementRead = vi.fn(
    async () => ready as import('../api/client').AgentLifecycleOperationView,
  );
  const replacement = agentClient({
    readAgentLifecycle: replacementRead,
    createAgentLifecycle: f.create,
  });
  view.rerender(<AgentLifecycleCard {...f.props} client={replacement} />);
  await advance(60_000);
  expect(f.fetcher).toHaveBeenCalledOnce();
  expect(replacementRead).toHaveBeenCalledOnce();
  expect(f.opened).toHaveBeenCalledOnce();
  expect(f.create).not.toHaveBeenCalled();
  view.unmount();
  await advance(0);
  expect(vi.getTimerCount()).toBe(0);
});
