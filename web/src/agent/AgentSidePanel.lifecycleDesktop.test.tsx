import { ApiError, createClient } from '@cma/client';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { taskExperience } from '../test/taskExperience';
import { computerFixture, live, paused } from './computerLifecycle.fixture';
import { environmentFixture } from './environmentReview.fixture';

const { connect } = vi.hoisted(() => ({ connect: vi.fn() }));
vi.mock('@novnc/novnc', () => ({ default: connect }));
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('omits browser enablement when Screen already provides a full desktop', async () => {
  const sdk = createClient({ baseUrl: 'https://fixture.invalid' });
  vi.spyOn(sdk.agents, 'desktop').mockImplementation(() => new Promise(() => {}));
  const f = computerFixture({ resourceViews: sdk, taskExperience: taskExperience('owner') });
  f.files.environment.mockResolvedValue({
    ...environmentFixture,
    browser: { ...environmentFixture.browser, state: 'not_enabled' },
  });
  render(
    f.draw({
      panel: 'browser',
      detail: { ...AGENT_DETAIL, desktop: true },
      resourceAvailability: live,
    }),
  );
  await waitFor(() => expect(f.files.environment).toHaveBeenCalledOnce());
  await act(async () => {});
  expect(screen.getByRole('region', { name: 'Machine desktop' })).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Enable browser' })).toBeNull();
  expect(f.browser.configure).not.toHaveBeenCalled();
});

it.each(['pause', 'closed', 'viewer', 'agent', 'runtime'] as const)(
  'cancels desktop discovery at %s and refuses the old connection',
  async (transition) => {
    const sdk = createClient({ baseUrl: 'https://fixture.invalid' });
    let finish!: (value: { websocket_url: string }) => void;
    const discover = vi
      .spyOn(sdk.agents, 'desktop')
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      )
      .mockImplementation(() => new Promise(() => {}));
    const f = computerFixture({ resourceViews: sdk });
    const props = {
      panel: 'browser' as const,
      detail: { ...AGENT_DETAIL, desktop: true },
      resourceAvailability: live,
    };
    const view = render(f.draw(props));
    await act(async () => {});
    const desktop = screen.getByRole('region', { name: 'Machine desktop' });
    const signal = discover.mock.calls[0]![1]!.signal!;
    const next =
      transition === 'pause'
        ? { resourceAvailability: paused }
        : transition === 'closed'
          ? { panel: undefined }
          : transition === 'viewer'
            ? { viewerId: 'another-owner' }
            : transition === 'agent'
              ? { agentId: 'agt_other' }
              : { resourceAvailability: { ...live, runtimeVersion: 'runtime-two' } };
    view.rerender(f.draw({ ...props, ...next }));
    expect(signal.aborted).toBe(true);
    await act(async () => finish({ websocket_url: '/api/v1/agents/agt_1/desktop/ws' }));
    expect(connect).not.toHaveBeenCalled();
    if (transition === 'pause') {
      expect(desktop).toBeInTheDocument();
      expect(desktop).not.toBeVisible();
      expect(desktop).toHaveAttribute('inert');
      expect(discover).toHaveBeenCalledOnce();
      view.rerender(f.draw(props));
      await act(async () => {});
      expect(screen.getByRole('region', { name: 'Machine desktop' })).toBe(desktop);
      expect(discover).toHaveBeenCalledTimes(2);
    }
    expect(f.mutations.reviveSandbox).not.toHaveBeenCalled();
    expect(f.mutations.stopAgent).not.toHaveBeenCalled();
  },
);

it.each(['stopped', 'deleted', 'retired'] as const)(
  'cancels pending retries when the Agent or Sandbox is %s',
  async (status) => {
    vi.useFakeTimers();
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    const sdk = createClient({ baseUrl: 'https://fixture.invalid' });
    const discover = vi
      .spyOn(sdk.agents, 'desktop')
      .mockRejectedValue(new ApiError(503, 'Synthetic failure', 'desktop_runtime_unavailable'));
    const f = computerFixture({ resourceViews: sdk });
    const props = {
      panel: 'browser' as const,
      detail: { ...AGENT_DETAIL, desktop: true },
      resourceAvailability: live,
    };
    const view = render(f.draw(props));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(discover).toHaveBeenCalledOnce();
    view.rerender(
      f.draw({
        ...props,
        detail:
          status === 'retired'
            ? { ...props.detail, sandbox: { ...props.detail.sandbox, status } }
            : { ...props.detail, status },
      }),
    );
    await act(() => vi.advanceTimersByTimeAsync(600_000));
    expect(discover).toHaveBeenCalledOnce();
    expect(connect).not.toHaveBeenCalled();
    expect(f.mutations.reviveSandbox).not.toHaveBeenCalled();
    expect(f.mutations.runAgent).not.toHaveBeenCalled();
    expect(f.mutations.stopAgent).not.toHaveBeenCalled();
  },
);
