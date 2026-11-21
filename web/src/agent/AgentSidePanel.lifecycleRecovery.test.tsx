import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentEnvironment } from '@cma/client';
import { computerFixture, live, paused } from './computerLifecycle.fixture';
import { environmentFixture } from './environmentReview.fixture';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('rechecks capabilities once on paused to live even when Agent readiness never changes', async () => {
  const f = computerFixture();
  const view = render(f.draw());
  await act(async () => {});
  expect(f.files.environment).not.toHaveBeenCalled();
  view.rerender(f.draw({ resourceAvailability: live }));
  await act(async () => {});
  expect(f.files.environment).toHaveBeenCalledOnce();
  expect(f.files.list).toHaveBeenCalledOnce();
  view.rerender(f.draw({ resourceAvailability: paused }));
  view.rerender(f.draw({ resourceAvailability: live }));
  await act(async () => {});
  expect(f.files.environment).toHaveBeenCalledTimes(2);
  expect(f.files.write).not.toHaveBeenCalled();
  expect(f.browser.configure).not.toHaveBeenCalled();
});

it.each(['pause', 'closed', 'viewer', 'agent', 'runtime'] as const)(
  'cancels pending capabilities at %s and refuses late success',
  async (transition) => {
    const f = computerFixture();
    let resolve!: (value: AgentEnvironment) => void;
    f.files.environment.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done;
        }),
    );
    f.files.environment.mockImplementation(() => new Promise(() => {}));
    const view = render(f.draw({ resourceAvailability: live }));
    await act(async () => {});
    const first = vi.mocked(f.client.files!.environment!).mock.calls[0]!;
    const props =
      transition === 'pause'
        ? { resourceAvailability: paused }
        : transition === 'closed'
          ? { panel: undefined }
          : transition === 'viewer'
            ? { viewerId: 'another-owner' }
            : transition === 'agent'
              ? { agentId: 'agt_other' }
              : { resourceAvailability: { ...live, runtimeVersion: 'runtime-two' } };
    view.rerender(f.draw({ resourceAvailability: live, ...props }));
    expect(first[1]?.signal?.aborted).toBe(true);
    await act(async () => resolve(environmentFixture));
    expect(f.files.list).not.toHaveBeenCalled();
    expect(f.browser.read).not.toHaveBeenCalled();
    expect(f.files.write).not.toHaveBeenCalled();
    if (transition === 'closed') {
      expect(f.files.environment).toHaveBeenCalledOnce();
      view.rerender(f.draw({ resourceAvailability: live }));
      await act(async () => {});
      expect(f.files.environment).toHaveBeenCalledTimes(2);
    }
    if (transition === 'viewer' || transition === 'agent') {
      view.rerender(f.draw({ ...props, resourceAvailability: paused, panel: 'activity' }));
      expect(screen.queryByText('retained-output', { exact: true })).toBeNull();
    }
  },
);

it.each(['unsupported', 'not_enabled'] as const)(
  'does not enable a %s browser after Resume',
  async (state) => {
    const f = computerFixture();
    f.files.environment.mockResolvedValue({
      ...environmentFixture,
      browser: {
        ...environmentFixture.browser,
        state,
      },
    });
    const view = render(f.draw({ panel: 'browser' }));
    view.rerender(f.draw({ panel: 'browser', resourceAvailability: live }));
    await act(async () => {});
    expect(
      screen.getByText(
        state === 'unsupported'
          ? 'Live screen is not supported by this environment.'
          : 'Live screen is not enabled',
      ),
    ).toBeVisible();
    expect(f.browser.configure).not.toHaveBeenCalled();
    expect(f.browser.read).not.toHaveBeenCalled();
  },
);

it('offers the parent navigation only with management permission and never resumes automatically', async () => {
  const f = computerFixture();
  const view = render(f.draw({ canManageSandbox: true }));
  fireEvent.click(screen.getByRole('button', { name: 'Open Sandbox to resume' }));
  expect(f.onOpenSandbox).toHaveBeenCalledOnce();
  expect(f.mutations.reviveSandbox).not.toHaveBeenCalled();
  view.rerender(f.draw({ canManageSandbox: false }));
  expect(screen.queryByRole('button', { name: 'Open Sandbox to resume' })).toBeNull();
  view.rerender(f.draw({ canManageSandbox: true, shared: true }));
  expect(screen.queryByRole('button', { name: 'Open Sandbox to resume' })).toBeNull();
  await act(async () => {});
  expect(f.files.environment).not.toHaveBeenCalled();
});
