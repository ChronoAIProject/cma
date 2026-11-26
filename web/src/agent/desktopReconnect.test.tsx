import { ApiError, createClient } from '@cma/client';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import { DesktopPanel } from './DesktopPanel';
import type { ComponentProps } from 'react';

// Clock timers are covered separately; these assertions isolate connection retries.
vi.mock('./DesktopLocalTime', () => ({ DesktopLocalTime: () => null }));

type Connection = EventTarget & {
  viewOnly: boolean;
  canvas: HTMLCanvasElement;
  painted: boolean;
  disconnect: ReturnType<typeof vi.fn>;
};
const { connections } = vi.hoisted(() => ({ connections: [] as Connection[] }));
vi.mock('@novnc/novnc', () => ({
  default: class extends EventTarget {
    viewOnly = true;
    canvas = document.createElement('canvas');
    painted = true;
    disconnect = vi.fn(() => this.canvas.remove());
    focus = vi.fn();
    blur = vi.fn();
    sendKey = vi.fn();
    constructor(target: HTMLElement) {
      super();
      Object.defineProperty(this.canvas, 'getContext', {
        value: () => ({ getImageData: () => ({ data: [0, 0, 0, this.painted ? 255 : 0] }) }),
      });
      target.append(this.canvas);
      connections.push(this);
    }
  },
}));

beforeEach(() => {
  connections.length = 0;
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  expect(vi.getTimerCount()).toBe(0);
  vi.useRealTimers();
  vi.restoreAllMocks();
});

/** A discovery that never answers until its attempt is aborted. */
const inFlight = Symbol('in flight');
function mount(failure?: unknown, defaults: Partial<ComponentProps<typeof DesktopPanel>> = {}) {
  const sdk = createClient({ baseUrl: 'https://fixture.invalid' });
  const discover = vi.spyOn(sdk.agents, 'desktop');
  if (failure === inFlight)
    discover.mockImplementation(
      (_agent, options) =>
        new Promise((_resolve, reject) =>
          options?.signal?.addEventListener('abort', () =>
            reject(new DOMException('Superseded', 'AbortError')),
          ),
        ),
    );
  else if (failure) discover.mockRejectedValue(failure);
  else discover.mockResolvedValue({ websocket_url: '/api/v1/agents/agt_retry/desktop/ws' });
  const client = agentClient({ resourceViews: sdk });
  const preparation = { context: {}, owner: 'owner' };
  const draw = (props: Partial<ComponentProps<typeof DesktopPanel>> = {}) => (
    <CmaClientProvider client={client}>
      <DesktopPanel
        runtimeReady
        agentId="agt_retry"
        running={false}
        onClose={vi.fn()}
        preparation={preparation}
        {...defaults}
        {...props}
      />
    </CmaClientProvider>
  );
  const view = render(draw());
  return {
    view,
    discover,
    redraw: (props: Partial<ComponentProps<typeof DesktopPanel>>) => view.rerender(draw(props)),
  };
}
function visible(value: boolean) {
  vi.spyOn(document, 'hidden', 'get').mockReturnValue(!value);
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue(value ? 'visible' : 'hidden');
  act(() => document.dispatchEvent(new Event('visibilitychange')));
}
const exhausted = 'Could not reconnect automatically. Choose Reconnect to try again.';
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

it('recovers from repeated 30-second stream drops followed by runtime unavailability', async () => {
  const { discover } = mount();
  await advance(0);
  for (let index = 0; index < 6; index++) {
    act(() => connections[index]!.dispatchEvent(new Event('connect')));
    await advance(30_000);
    if (index === 5)
      discover.mockRejectedValueOnce(
        new ApiError(503, 'Synthetic failure', 'desktop_runtime_unavailable'),
      );
    act(() => connections[index]!.dispatchEvent(new Event('disconnect')));
    await advance(Math.min(30_000, 1_000 * 2 ** index));
  }
  expect(discover).toHaveBeenCalledTimes(7);
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  expect(screen.queryByText('Automatic reconnect attempts have stopped')).not.toBeInTheDocument();
  await advance(30_000);
  expect(discover).toHaveBeenCalledTimes(8);
  connections[6]!.painted = false;
  act(() => connections[6]!.dispatchEvent(new Event('connect')));
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  connections[6]!.painted = true;
  await advance(50);
  expect(screen.getByRole('status')).toHaveTextContent('Connected');
  expect(connections[6]!.viewOnly).toBe(true);
});

it('keeps the reconnect status while a refresh briefly withholds the runtime version', async () => {
  const { discover, redraw } = mount(new ApiError(503, 'Synthetic failure'), {
    runtimeVersion: 'runtime-one',
  });
  await advance(0);
  expect(discover).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  // An availability refresh withholds the version, then restores the same one.
  // The retry is still waiting out its backoff; nothing is connecting yet.
  redraw({ runtimeVersion: undefined });
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  redraw({ runtimeVersion: 'runtime-one' });
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  redraw({ runtimeReady: false, runtimeVersion: undefined });
  expect(screen.getByRole('status')).toHaveTextContent('Waiting');
  redraw({ runtimeReady: true, runtimeVersion: 'runtime-one' });
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  expect(discover).toHaveBeenCalledTimes(1);
  await advance(1000);
  expect(discover).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
});

it('does not spend the automatic budget when a refresh supersedes an attempt in flight', async () => {
  const { discover, redraw } = mount(inFlight, { runtimeVersion: 'runtime-one' });
  await advance(0);
  expect(discover).toHaveBeenCalledTimes(1);
  // Availability refreshes withhold and restore the same runtime while the
  // discovery is still unanswered. Each rebuild restarts that same attempt;
  // nothing failed, so none of them is an automatic retry (#3295).
  for (let refresh = 0; refresh < 8; refresh++) {
    redraw({ runtimeVersion: undefined });
    await advance(0);
    redraw({ runtimeVersion: 'runtime-one' });
    await advance(0);
  }
  expect(screen.getByRole('status')).toHaveTextContent('Connecting to your machine…');
  expect(screen.getByRole('status')).not.toHaveTextContent(exhausted);
});

it('stops only when the five-minute retry budget expires with no loading timers', async () => {
  const { discover } = mount(new ApiError(503, 'Synthetic failure'));
  await advance(299_999);
  expect(discover).toHaveBeenCalledTimes(14);
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  await advance(1);
  expect(screen.getByRole('status')).toHaveTextContent(
    'Could not reconnect automatically. Choose Reconnect to try again.',
  );
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  expect(vi.getTimerCount()).toBe(0);
  await advance(600_000);
  expect(discover).toHaveBeenCalledTimes(14);
});

it.each([undefined, 'desktop_unavailable', 'not_found'])(
  'stops on every HTTP 404 (%s) and allows explicit retry',
  async (code) => {
    const { discover } = mount(new ApiError(404, '<html>private detail</html>', code));
    await advance(0);
    expect(screen.getByRole('status')).toHaveTextContent(
      'Desktop is currently unavailable. Reconnect when it becomes available.',
    );
    expect(document.body).not.toHaveTextContent('private detail');
    expect(vi.getTimerCount()).toBe(0);
    await advance(600_000);
    expect(discover).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
    await advance(0);
    expect(discover).toHaveBeenCalledTimes(2);
  },
);

it.each([401, 403])('stops on %s without claiming to reconnect', async (status) => {
  const { discover } = mount(new ApiError(status, 'private service error'));
  await advance(0);
  expect(screen.getByRole('status')).toHaveTextContent('Desktop access was denied.');
  expect(screen.getByRole('status')).not.toHaveTextContent('Reconnecting');
  visible(false);
  await advance(600_000);
  visible(true);
  visible(true);
  await advance(600_000);
  expect(discover).toHaveBeenCalledTimes(1);
  expect(vi.getTimerCount()).toBe(0);
});

it.each([
  new ApiError(503, 'Synthetic failure', 'desktop_runtime_unavailable'),
  new ApiError(503, 'Synthetic failure', 'desktop_unavailable'),
  new Error('desktop_unavailable'),
  { status: 404, code: 'desktop_unavailable' },
])('retries transient and untyped errors without inferring a refusal: %s', async (failure) => {
  const { discover } = mount(failure);
  await advance(1_000);
  expect(discover).toHaveBeenCalledTimes(2);
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  await advance(299_000);
  expect(discover).toHaveBeenCalledTimes(14);
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
});

it.each([
  new ApiError(404, 'Synthetic failure', 'desktop_unavailable'),
  new ApiError(503, 'Synthetic failure'),
])('retains a terminal budget across visibility and same-scope effects: %s', async (failure) => {
  const { discover, redraw } = mount(failure);
  await advance(300_000);
  const calls = discover.mock.calls.length;
  const message = screen.getByRole('status').textContent;
  visible(false);
  visible(true);
  visible(true);
  redraw({ preparation: { context: {}, owner: 'owner' } });
  await advance(600_000);
  expect(discover).toHaveBeenCalledTimes(calls);
  expect(screen.getByRole('status').textContent).toBe(message);
  expect(vi.getTimerCount()).toBe(0);
  expect(screen.getAllByRole('status')).toHaveLength(1);
  expect(document.querySelector('.agent-desktop__loading')).toHaveAttribute('aria-hidden', 'true');
  expect(document.querySelector('.agent-desktop__loading')).toHaveTextContent(message!);
  expect(document.querySelector('.agent-desktop__placeholder')).toBeNull();
});

it('makes no requests while hidden and preserves backoff across same-scope effects', async () => {
  const { discover, redraw } = mount(new ApiError(503, 'Synthetic failure'));
  await advance(0);
  visible(false);
  await advance(10_000);
  expect(discover).toHaveBeenCalledTimes(1);
  visible(true);
  visible(true);
  await advance(0);
  expect(discover).toHaveBeenCalledTimes(2);
  for (let i = 0; i < 10; i++) {
    visible(true);
    redraw({ preparation: { context: {}, owner: 'owner' } });
  }
  await advance(1_999);
  expect(discover).toHaveBeenCalledTimes(2);
  await advance(1);
  expect(discover).toHaveBeenCalledTimes(3);
  await advance(600_000);
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
});

it('does not renew an expired budget when the hidden panel becomes visible', async () => {
  const { discover } = mount(new ApiError(503, 'Synthetic failure'));
  await advance(0);
  visible(false);
  await advance(300_000);
  visible(true);
  await advance(0);
  expect(discover).toHaveBeenCalledOnce();
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
});

it('ends recovery at the budget deadline even when Retry-After exceeds it', async () => {
  const { discover } = mount(new ApiError(503, 'Busy', undefined, undefined, 600_000));
  await advance(299_999);
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  await advance(1);
  expect(discover).toHaveBeenCalledOnce();
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
  expect(vi.getTimerCount()).toBe(0);
});

it('aborts discovery at budget expiry and ignores its late success', async () => {
  const { discover } = mount(new ApiError(503, 'Synthetic failure'));
  await advance(241_000);
  let finish!: (value: { websocket_url: string }) => void;
  discover.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await advance(30_000);
  const signal = discover.mock.calls.at(-1)![1]!.signal!;
  expect(signal.aborted).toBe(false);
  await advance(29_000);
  expect(signal.aborted).toBe(true);
  await act(async () => finish({ websocket_url: '/api/v1/agents/agt_retry/desktop/ws' }));
  expect(connections).toHaveLength(0);
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
  expect(vi.getTimerCount()).toBe(0);
});

it.each([409, 429])(
  'resets an exhausted budget manually without shortening the final %s cooldown',
  async (status) => {
    const { discover } = mount(new ApiError(503, 'Synthetic failure'));
    await advance(241_000);
    expect(discover).toHaveBeenCalledTimes(13);
    discover.mockRejectedValueOnce(new ApiError(status, 'Busy', 'desktop_busy', undefined, 90_000));
    await advance(59_000);
    expect(discover).toHaveBeenCalledTimes(14);
    expect(screen.getByRole('status')).toHaveTextContent(exhausted);
    fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
    visible(false);
    await advance(60_999);
    visible(true);
    visible(true);
    expect(discover).toHaveBeenCalledTimes(14);
    await advance(1);
    expect(discover).toHaveBeenCalledTimes(15);
    await advance(300_000);
    expect(discover).toHaveBeenCalledTimes(28);
    expect(screen.getByRole('status')).toHaveTextContent(exhausted);
  },
);

it('recovers manually after exhaustion with a fresh view-only stream', async () => {
  const { discover } = mount();
  await advance(0);
  act(() => connections[0]!.dispatchEvent(new Event('connect')));
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(connections[0]!.viewOnly).toBe(false);
  discover.mockRejectedValue(new ApiError(503, 'Synthetic failure'));
  act(() => connections[0]!.dispatchEvent(new Event('disconnect')));
  await advance(300_000);
  expect(discover).toHaveBeenCalledTimes(14);
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
  discover.mockResolvedValue({ websocket_url: '/api/v1/agents/agt_retry/desktop/ws' });
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
  await advance(1_000);
  expect(discover).toHaveBeenCalledTimes(15);
  act(() => connections[1]!.dispatchEvent(new Event('connect')));
  expect(screen.getByRole('status')).toHaveTextContent('Connected');
  expect(connections[1]!.viewOnly).toBe(true);
  expect(connections[0]!.disconnect).toHaveBeenCalledOnce();
});

it.each([59_999, 60_000])(
  'only resets after 60 seconds continuously ready: %s ms',
  async (readyMs) => {
    const { discover } = mount(new ApiError(503, 'Synthetic failure'));
    await advance(241_000);
    discover.mockResolvedValueOnce({ websocket_url: '/api/v1/agents/agt_retry/desktop/ws' });
    await advance(30_000);
    act(() => connections[0]!.dispatchEvent(new Event('connect')));
    await advance(readyMs);
    // The budget deadline must not tear down a painted stream before it can stabilize.
    expect(connections[0]!.disconnect).not.toHaveBeenCalled();
    act(() => connections[0]!.dispatchEvent(new Event('disconnect')));
    if (readyMs === 59_999) {
      expect(screen.getByRole('status')).toHaveTextContent(exhausted);
      await advance(600_000);
      expect(discover).toHaveBeenCalledTimes(14);
      expect(vi.getTimerCount()).toBe(0);
    } else {
      expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
      await advance(300_000);
      expect(discover).toHaveBeenCalledTimes(27);
      expect(screen.getByRole('status')).toHaveTextContent(exhausted);
    }
  },
);

it.each(['owner', 'agent', 'runtime', 'pause'] as const)(
  'allows a new budget after a real %s scope change',
  async (scope) => {
    const { discover, redraw } = mount(
      new ApiError(404, 'Synthetic failure', 'desktop_unavailable'),
    );
    await advance(0);
    if (scope === 'pause') {
      redraw({ runtimeReady: false });
      await advance(600_000);
      expect(discover).toHaveBeenCalledTimes(1);
      redraw({ runtimeReady: true });
    } else
      redraw(
        scope === 'owner'
          ? { preparation: { context: {}, owner: 'next' } }
          : scope === 'agent'
            ? { agentId: 'agt_next' }
            : { runtimeVersion: 'runtime-2' },
      );
    await advance(0);
    expect(discover).toHaveBeenCalledTimes(2);
  },
);

it('limits missing-first-frame recovery with no surviving stream or timer', async () => {
  const { discover } = mount();
  await advance(330_000);
  expect(discover).toHaveBeenCalledTimes(8);
  expect(connections).toHaveLength(8);
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
  expect(vi.getTimerCount()).toBe(0);
  for (const connection of connections) expect(connection.disconnect).toHaveBeenCalledOnce();
});

it('counts lost canvases against the same budget and revokes input on every recovery', async () => {
  const { discover } = mount();
  await advance(0);
  for (let i = 0; i < 14; i++) {
    expect(connections[i]!.viewOnly).toBe(true);
    act(() => connections[i]!.dispatchEvent(new Event('connect')));
    fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
    expect(connections[i]!.viewOnly).toBe(false);
    act(() => connections[i]!.canvas.dispatchEvent(new Event('contextlost', { bubbles: true })));
    expect(connections[i]!.viewOnly).toBe(true);
    if (i < 13) await advance(Math.min(30_000, 1_000 * 2 ** i));
  }
  expect(screen.getByRole('status')).toHaveTextContent('Reconnecting');
  await advance(29_000);
  expect(discover).toHaveBeenCalledTimes(14);
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
  // The prior control notice may finish its 200 ms exit animation.
  await advance(200);
  expect(vi.getTimerCount()).toBe(0);
});

it('keeps noVNC security failure terminal through effect rebuilds', async () => {
  const { discover, redraw } = mount();
  await advance(0);
  act(() => connections[0]!.dispatchEvent(new Event('securityfailure')));
  redraw({ preparation: { context: {}, owner: 'owner' } });
  visible(false);
  visible(true);
  await advance(600_000);
  expect(discover).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('status')).toHaveTextContent('Desktop connection refused.');
  expect(vi.getTimerCount()).toBe(0);
});

it('preserves a terminal budget while authority temporarily withholds the same runtime', async () => {
  const { discover, redraw } = mount(new ApiError(503, 'Synthetic failure'), {
    runtimeVersion: 'runtime-one',
  });
  await advance(300_000);
  expect(discover).toHaveBeenCalledTimes(14);
  redraw({ runtimeReady: false, runtimeVersion: undefined });
  await advance(600_000);
  redraw({ runtimeReady: true, runtimeVersion: 'runtime-one' });
  await advance(600_000);
  expect(discover).toHaveBeenCalledTimes(14);
  expect(screen.getByRole('status')).toHaveTextContent(exhausted);
  expect(vi.getTimerCount()).toBe(0);
  redraw({ runtimeReady: true, runtimeVersion: 'runtime-two' });
  await advance(0);
  expect(discover).toHaveBeenCalledTimes(15);
});
