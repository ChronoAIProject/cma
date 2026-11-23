import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import type { CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { parkedRuntimeResultFixture, sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { OwnerWorkspacePage } from '../resources/OwnerWorkspacePage';
import { WherePicker } from '../create/WherePicker';
import { SandboxPage } from './SandboxPage';
import { LaunchSandboxProgress } from './LaunchSandboxProgress';
import { SandboxActionStatus } from './SandboxActionStatus';

const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));
const permissions = {
  createWorkspace: false,
  createAgent: false,
  pauseSandbox: false,
  deleteSandbox: false,
  removeWorkspace: false,
  stopAgent: false,
  deleteAgent: false,
  exportHistory: true,
};
beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

for (const surface of ['Sandbox', 'Workspace', 'Picker', 'Launch'] as const) {
  it.each(['image_rotation', 'revive'] as const)(
    `${surface} observes parked %s and its late events at five seconds despite preparing children`,
    async (purpose) => {
      const workspace = { ...workspaceFixture(), status: 'preparing' as const };
      let sandbox = { ...parkedRuntimeResultFixture(purpose), workspaces: [workspace] };
      const readSandbox = vi.fn<CmaWebClient['readSandbox']>(async () => sandbox);
      const readSandboxEvents = vi.fn(async () => []);
      const client = agentClient({
        readSandbox,
        readWorkspace: async () => workspace,
        readSandboxEvents,
      });
      const common = {
        client,
        revision: 0,
        permissions,
        busy: false,
        onAction: vi.fn(),
        onNavigate: vi.fn(),
      };
      const view = render(
        surface === 'Sandbox' ? (
          <SandboxPage {...common} sandboxId="sbx_1" />
        ) : surface === 'Workspace' ? (
          <OwnerWorkspacePage {...common} workspaceId="wsp_1" />
        ) : surface === 'Picker' ? (
          <WherePicker
            client={client}
            value={{ sandboxId: 'sbx_1', workspaceId: 'wsp_1' }}
            onChange={vi.fn()}
            onReady={vi.fn()}
          />
        ) : (
          <CmaClientProvider client={client}>
            <LaunchSandboxProgress sandboxId="sbx_1" />
          </CmaClientProvider>
        ),
      );
      await tick();
      if (surface === 'Sandbox') {
        fireEvent.click(screen.getByRole('tab', { name: 'Activity' }));
        await tick();
      }
      const events = readSandboxEvents.mock.calls.length;
      expect(readSandbox).toHaveBeenCalledTimes(1);
      if (surface === 'Workspace') {
        expect(
          screen.queryByText(purpose === 'revive' ? 'Resuming' : 'Updating runtime'),
        ).not.toBeInTheDocument();
      }
      await tick(4999);
      expect(readSandbox).toHaveBeenCalledTimes(1);
      expect(readSandboxEvents).toHaveBeenCalledTimes(events);
      await tick(1);
      expect(readSandbox).toHaveBeenCalledTimes(2);
      if (surface !== 'Workspace')
        expect(readSandboxEvents.mock.calls.length).toBeGreaterThan(events);
      sandbox = { ...sandboxFixture(), workspaces: [] };
      await tick(5000);
      const signal = readSandbox.mock.calls.at(-1)?.[1]?.signal;
      view.unmount();
      expect(signal?.aborted).toBe(true);
      const count = readSandbox.mock.calls.length;
      await tick(60_000);
      expect(readSandbox).toHaveBeenCalledTimes(count);
    },
  );
}

it('an accepted resume stops fast waiting on the generic parked result without declaring success', async () => {
  const readSandbox = vi.fn(async () => parkedRuntimeResultFixture('revive'));
  const onSettled = vi.fn();
  render(
    <SandboxActionStatus
      client={agentClient({ readSandbox })}
      request={{
        action: { kind: 'revive_sandbox', sandboxId: 'sbx_1' },
        key: 'original-intent',
        phase: 'accepted',
      }}
      onSettled={onSettled}
      onClose={vi.fn()}
    />,
  );
  await tick();
  expect(onSettled).toHaveBeenCalledExactlyOnceWith(false);
  expect(screen.getByRole('alert')).toHaveTextContent('runtime update needs attention');
  await tick(60_000);
  expect(readSandbox).toHaveBeenCalledTimes(1);
});

it('parked Launch observation respects a longer Retry-After and cancels a pending read', async () => {
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(parkedRuntimeResultFixture())
    .mockRejectedValueOnce(new ApiError(503, 'Unavailable', undefined, undefined, 12_000))
    .mockResolvedValue(parkedRuntimeResultFixture());
  const view = render(
    <CmaClientProvider client={agentClient({ readSandbox })}>
      <LaunchSandboxProgress sandboxId="sbx_1" />
    </CmaClientProvider>,
  );
  await tick();
  await tick(5000);
  fireEvent(document, new Event('visibilitychange'));
  await tick();
  await tick(11_999);
  expect(readSandbox).toHaveBeenCalledTimes(2);
  await tick(1);
  expect(readSandbox).toHaveBeenCalledTimes(3);
  view.unmount();
  await tick(60_000);
  expect(readSandbox).toHaveBeenCalledTimes(3);
});
