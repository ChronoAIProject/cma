import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { WIRE_SANDBOX_DETAIL } from '../test/rest';
import { SandboxAccessPage } from './SandboxAccessPage';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
function fixture(failure: 'access' | 'detail' = 'access') {
  vi.useFakeTimers();
  const state = { status: 503, retryAfter: undefined as string | undefined };
  const paths: string[] = [];
  const fetch = vi.fn<typeof globalThis.fetch>(async (input, init) => {
    expect(init?.method ?? 'GET').toBe('GET');
    const path = new URL(String(input)).pathname;
    paths.push(path);
    const access = path.includes('/resource-access/');
    if (state.status && access === (failure === 'access'))
      return Response.json(
        { status: state.status, detail: 'Fixture read failed' },
        {
          status: state.status,
          headers: state.retryAfter ? { 'Retry-After': state.retryAfter } : {},
        },
      );
    return Response.json(
      access
        ? {
            capabilities: ['view_details'],
            policy_revision: 1,
            revalidate_at: null,
          }
        : { ...WIRE_SANDBOX_DETAIL, status: 'retired' },
    );
  });
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const draw = (id = WIRE_SANDBOX_DETAIL.id) => (
    <SandboxAccessPage
      client={client}
      id={id}
      personal={<p>Fresh owner environment</p>}
      onNavigate={vi.fn()}
    />
  );
  return { state, paths, draw, fetch };
}

for (const failure of ['access', 'detail'] as const) {
  it.each(['manual', 'online', 'scheduled'] as const)(
    `recovers ${failure} 503 by %s with fresh authorization and detail reads`,
    async (recovery) => {
      const f = fixture(failure);
      render(f.draw());
      await act(async () => {});
      expect(screen.getByRole('heading', { name: 'Sandbox unavailable' })).toBeVisible();
      expect(screen.queryByText('Fresh owner environment')).not.toBeInTheDocument();
      f.state.status = 0;
      const before = f.paths.length;
      await act(async () => {
        if (recovery === 'manual')
          fireEvent.click(screen.getByRole('button', { name: 'Retry environment check' }));
        else if (recovery === 'online') fireEvent(window, new Event('online'));
        else await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(screen.getByText('Fresh owner environment')).toBeVisible();
      expect(f.paths.slice(before)).toEqual([
        `/api/v1/resource-access/sandbox/${WIRE_SANDBOX_DETAIL.id}`,
        `/api/v1/sandboxes/${WIRE_SANDBOX_DETAIL.id}`,
        `/api/v1/resource-access/sandbox/${WIRE_SANDBOX_DETAIL.id}`,
      ]);
    },
  );
}

it('keeps a repeated read failure unknown without a new polling loop', async () => {
  const f = fixture();
  render(f.draw());
  await act(async () => {});
  await act(async () =>
    fireEvent.click(screen.getByRole('button', { name: 'Retry environment check' })),
  );
  await act(async () => fireEvent(window, new Event('online')));
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(f.fetch).toHaveBeenCalledTimes(3);
  expect(screen.getByRole('heading', { name: 'Sandbox unavailable' })).toBeVisible();
  expect(screen.queryByText('Fresh owner environment')).not.toBeInTheDocument();
});

it.each([401, 403, 404])('does not treat %s as a transient recovery signal', async (status) => {
  const f = fixture();
  f.state.status = status;
  render(f.draw());
  await act(async () => {});
  await act(async () => fireEvent(window, new Event('online')));
  expect(f.fetch).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('button', { name: 'Check Sandbox access' })).toBeVisible();
  expect(screen.queryByText('Fresh owner environment')).not.toBeInTheDocument();
});

it.each([429, 503])(
  'preserves %s Retry-After across explicit retry and online recovery',
  async (status) => {
    const f = fixture();
    vi.spyOn(Math, 'random').mockReturnValue(0);
    f.state.status = status;
    f.state.retryAfter = '20';
    render(f.draw());
    await act(async () => {});
    f.state.status = 0;
    for (let i = 0; i < 3; i++)
      await act(async () => {
        fireEvent.click(screen.getByRole('button', { name: 'Retry environment check' }));
        fireEvent(window, new Event('online'));
      });
    await act(() => vi.advanceTimersByTimeAsync(19_999));
    expect(f.fetch).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(screen.getByText('Fresh owner environment')).toBeVisible();
    expect(f.fetch).toHaveBeenCalledTimes(4);
  },
);

it('discards an older recovery when the resource changes to a denied one', async () => {
  const f = fixture();
  const view = render(f.draw());
  await act(async () => {});
  let finish!: (value: Response) => void;
  f.fetch.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await act(async () => fireEvent(window, new Event('online')));
  f.state.status = 403;
  view.rerender(f.draw('sandbox-other'));
  await act(async () => {});
  await act(async () =>
    finish(
      Response.json({ capabilities: ['view_details'], policy_revision: 1, revalidate_at: null }),
    ),
  );
  expect(screen.getByRole('heading', { name: 'Sandbox unavailable' })).toBeVisible();
  expect(screen.queryByText('Fresh owner environment')).not.toBeInTheDocument();
  expect(f.paths.some((path) => path.startsWith('/api/v1/sandboxes/'))).toBe(false);
});
