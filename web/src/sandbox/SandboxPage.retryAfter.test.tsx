import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { createRestWebClient } from '../api/rest';
import type { CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { WIRE_SANDBOX_DETAIL } from '../test/rest';
import { SandboxPage } from './SandboxPage';

const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));
const sandboxId = WIRE_SANDBOX_DETAIL.id;
const foreground = () => fireEvent(document, new Event('visibilitychange'));

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function fixture(status: 429 | 503, initialFailure = true) {
  vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout', 'performance'] });
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  let fail = initialFailure;
  let sandbox = { name: 'Atlas', revision: 8, pauseAllowed: true };
  const fetch = vi.fn<typeof globalThis.fetch>(async (input) => {
    const path = new URL(String(input)).pathname;
    expect(path).toMatch(/^\/api\/v1\/sandboxes\/[^/]+$/);
    const id = path.split('/').at(-1);
    if (id === sandboxId && fail) {
      return Response.json(
        { status, detail: 'Fixture temporarily unavailable' },
        { status, headers: { 'Retry-After': '20' } },
      );
    }
    return Response.json({
      ...WIRE_SANDBOX_DETAIL,
      id,
      name: id === sandboxId ? sandbox.name : 'Second Sandbox',
      revision: sandbox.revision,
      lifecycle: { pause: { allowed: sandbox.pauseAllowed }, revive: { allowed: false } },
    });
  });
  const wire = createRestWebClient(createClient({ baseUrl: 'https://fixture.invalid', fetch }));
  const client = agentClient({ readSandbox: wire.readSandbox });
  const onAction = vi.fn();
  const draw = (
    id = sandboxId,
    revision = 0,
    viewerId = 'owner',
    source: CmaWebClient = client,
  ) => (
    <SandboxPage
      client={source}
      sandboxId={id}
      revision={revision}
      viewerId={viewerId}
      permissions={{
        createWorkspace: false,
        createAgent: false,
        pauseSandbox: true,
        deleteSandbox: true,
        removeWorkspace: false,
        stopAgent: false,
        deleteAgent: false,
        exportHistory: true,
      }}
      busy={false}
      onAction={onAction}
      onNavigate={vi.fn()}
    />
  );
  return {
    draw,
    fetch,
    hidden,
    onAction,
    observe(next: typeof sandbox) {
      sandbox = next;
    },
    fail(value: boolean) {
      fail = value;
    },
  };
}

it.each([429, 503] as const)(
  'honors the HTTP %i floor across repeated foreground and manual retries',
  async (status) => {
    const f = fixture(status);
    render(f.draw());
    await tick();
    expect(screen.getByRole('heading', { name: 'Sandbox unavailable' })).toBeVisible();
    f.fail(false);
    await tick(5000);
    expect(f.fetch).toHaveBeenCalledTimes(1);
    for (let i = 0; i < 3; i++) {
      fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
      foreground();
      await tick(4000);
      expect(f.fetch).toHaveBeenCalledTimes(1);
    }
    await tick(2999);
    expect(f.fetch).toHaveBeenCalledTimes(1);
    foreground();
    await tick(1);
    expect(f.fetch).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('heading', { name: 'Atlas' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
    expect(f.fetch.mock.calls.map(([, init]) => init?.method ?? 'GET')).toEqual(['GET', 'GET']);
    expect(f.onAction).not.toHaveBeenCalled();
  },
);

it('keeps revision 8 stale after the delayed read returns revision 7, then recovers at 9', async () => {
  const f = fixture(503, false);
  const view = render(f.draw());
  await tick();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  f.fail(true);
  await tick(5000);
  expect(screen.getByText(/Progress could not be refreshed/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  view.rerender(f.draw(sandboxId, 1));
  await tick(19_999);
  expect(f.fetch).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('heading', { name: 'Atlas' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
  f.fail(false);
  f.observe({ name: 'Outdated Sandbox', revision: 7, pauseAllowed: false });
  await tick(1);
  expect(f.fetch).toHaveBeenCalledTimes(3);
  expect(screen.getByRole('heading', { name: 'Atlas' })).toBeVisible();
  const pause = screen.getByRole('button', { name: 'Pause' });
  expect(pause).toBeDisabled();
  fireEvent.click(pause);
  expect(f.onAction).not.toHaveBeenCalled();

  f.observe({ name: 'Confirmed Sandbox', revision: 9, pauseAllowed: true });
  await tick(5000);
  expect(f.fetch).toHaveBeenCalledTimes(4);
  expect(screen.getByRole('heading', { name: 'Confirmed Sandbox' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
  expect(f.onAction).not.toHaveBeenCalled();
});

it('waits for foreground if the server delay expires while the page is hidden', async () => {
  const f = fixture(503);
  render(f.draw());
  await tick();
  f.fail(false);
  f.hidden.mockReturnValue(true);
  foreground();
  await tick(20_000);
  expect(f.fetch).toHaveBeenCalledTimes(1);
  f.hidden.mockReturnValue(false);
  foreground();
  foreground();
  await tick();
  expect(f.fetch).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('heading', { name: 'Atlas' })).toBeVisible();
  expect(f.onAction).not.toHaveBeenCalled();
});

it('drops the old timer on a Sandbox switch and cancels the new timer on unmount', async () => {
  const f = fixture(429);
  const view = render(f.draw());
  await tick();
  view.rerender(f.draw('sandbox-2'));
  await tick();
  expect(screen.getByRole('heading', { name: 'Second Sandbox' })).toBeVisible();
  expect(f.fetch).toHaveBeenCalledTimes(2);

  view.rerender(f.draw());
  await tick();
  expect(f.fetch).toHaveBeenCalledTimes(3);
  view.unmount();
  foreground();
  await tick(60_000);
  expect(f.fetch).toHaveBeenCalledTimes(3);
});

it.each(['viewer', 'client'] as const)(
  'keeps the retry floor within one %s scope',
  async (scope) => {
    const f = fixture(429);
    const view = render(f.draw());
    await tick();
    f.fail(false);
    view.rerender(
      scope === 'viewer'
        ? f.draw(sandboxId, 0, 'second-owner')
        : f.draw(
            sandboxId,
            0,
            'owner',
            agentClient({
              readSandbox: async () => ({
                ...sandboxFixture(),
                id: sandboxId,
                name: 'New client Sandbox',
              }),
            }),
          ),
    );
    await tick();
    expect(
      screen.getByRole('heading', { name: scope === 'viewer' ? 'Atlas' : 'New client Sandbox' }),
    ).toBeVisible();
    expect(f.onAction).not.toHaveBeenCalled();
  },
);
