import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { OwnerWorkspacePage } from '../resources/OwnerWorkspacePage';
import { SandboxPage } from './SandboxPage';

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0.5);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const permissions = {
  createWorkspace: true,
  createAgent: true,
  pauseSandbox: true,
  deleteSandbox: true,
  removeWorkspace: true,
  stopAgent: true,
  deleteAgent: true,
  exportHistory: true,
};
const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

function fixture(surface: 'Sandbox' | 'Workspace') {
  let sandbox = { ...sandboxFixture('credential_rotating'), workspaces: [] };
  let workspace = workspaceFixture();
  const starts: number[] = [];
  const readSandbox = vi.fn(async () => {
    starts.push(Date.now());
    return sandbox;
  });
  const readWorkspace = vi.fn(async () => workspace);
  const client = agentClient({ readSandbox, readWorkspace });
  const draw = (revision = 0) => {
    const props = {
      client,
      revision,
      permissions,
      busy: false,
      onAction: vi.fn(),
      onNavigate: vi.fn(),
    };
    return surface === 'Sandbox' ? (
      <SandboxPage {...props} sandboxId="sbx_1" />
    ) : (
      <OwnerWorkspacePage {...props} workspaceId="wsp_1" />
    );
  };
  return {
    draw,
    starts,
    readSandbox,
    readWorkspace,
    change: () => {
      sandbox = { ...sandboxFixture('rotating'), workspaces: [] };
    },
    changeWorkspace: () => {
      workspace = { ...workspace, status: 'preparing' };
    },
    name: surface === 'Sandbox' ? 'Atlas' : 'chrono-bot',
  };
}

for (const surface of ['Sandbox', 'Workspace'] as const) {
  it(`${surface} backs off unchanged observations for twelve minutes and resets on observed progress`, async () => {
    const f = fixture(surface);
    const start = Date.now();
    render(f.draw());
    await tick(720_000);
    expect(f.starts.slice(0, 4).map((time) => time - start)).toEqual([0, 1500, 4500, 9500]);
    const gaps = f.starts.slice(1).map((time, i) => time - f.starts[i]!);
    expect(gaps).toContain(5000);
    expect(gaps).toContain(15_000);
    expect(gaps).toContain(30_000);
    for (let minute = 0; minute < 12; minute++) {
      const count = f.starts.filter(
        (time) => time >= start + minute * 60_000 && time < start + (minute + 1) * 60_000,
      ).length;
      expect(count).toBeLessThanOrEqual(minute >= 10 ? 3 : minute >= 2 ? 5 : 14);
    }
    f.change();
    await tick(f.starts.at(-1)! + 30_000 - Date.now());
    const count = f.starts.length;
    await tick(1499);
    expect(f.starts).toHaveLength(count);
    await tick(1);
    expect(f.starts).toHaveLength(count + 1);
  });

  it(`${surface} resets on visibility and explicit revision without overlapping reads`, async () => {
    const f = fixture(surface);
    const view = render(f.draw());
    await tick(130_000);
    const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    const before = f.readSandbox.mock.calls.length;
    await tick(60_000);
    expect(f.readSandbox).toHaveBeenCalledTimes(before);
    hidden.mockReturnValue(false);
    await act(async () => document.dispatchEvent(new Event('visibilitychange')));
    expect(f.readSandbox).toHaveBeenCalledTimes(before + 1);
    await tick(1500);
    expect(f.readSandbox).toHaveBeenCalledTimes(before + 2);
    view.rerender(f.draw(1));
    await tick();
    expect(f.readSandbox).toHaveBeenCalledTimes(before + 3);
    await tick(1500);
    expect(f.readSandbox).toHaveBeenCalledTimes(before + 4);
    f.readSandbox.mockImplementation(() => new Promise(() => {}));
    await tick(3000);
    const pending = f.readSandbox.mock.calls.length;
    await act(async () => {
      for (let i = 0; i < 10; i++) document.dispatchEvent(new Event('visibilitychange'));
    });
    await tick(90_000);
    expect(f.readSandbox).toHaveBeenCalledTimes(pending);
    view.unmount();
    await tick(90_000);
    expect(f.readSandbox).toHaveBeenCalledTimes(pending);
  });

  it(`${surface} retains the last observation after a transient error and retries after five seconds`, async () => {
    const f = fixture(surface);
    render(f.draw());
    await tick();
    f.readSandbox.mockRejectedValueOnce(new Error('Temporarily unavailable'));
    await tick(1500);
    expect(screen.getByRole('heading', { level: 1, name: f.name })).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent('Reconnecting');
    await tick(4999);
    expect(f.readSandbox).toHaveBeenCalledTimes(2);
    await tick(1);
    expect(f.readSandbox).toHaveBeenCalledTimes(3);
    await tick(200); // Let the existing inert exit animation finish.
    expect(screen.queryByText(/Reconnecting/)).not.toBeInTheDocument();
  });

  it.each([401, 403, 404, 410])(
    `${surface} withdraws denied detail (%s) and explicit retry resets readiness`,
    async (status) => {
      const f = fixture(surface);
      render(f.draw());
      await tick(130_000);
      f.readSandbox.mockRejectedValueOnce(new ApiError(status, 'Access unavailable'));
      await tick(f.starts.at(-1)! + 15_000 - Date.now());
      expect(screen.getByRole('heading', { name: `${surface} unavailable` })).toBeVisible();
      expect(screen.queryByRole('heading', { level: 1, name: f.name })).not.toBeInTheDocument();
      const count = f.readSandbox.mock.calls.length;
      fireEvent.click(screen.getByRole('button', { name: /Try again/ }));
      await tick();
      expect(f.readSandbox).toHaveBeenCalledTimes(count + 1);
      await tick(1500);
      expect(f.readSandbox).toHaveBeenCalledTimes(count + 2);
    },
  );
}

it('Workspace status progress resets the paired Sandbox cadence', async () => {
  const f = fixture('Workspace');
  render(f.draw());
  await tick(130_000);
  f.changeWorkspace();
  await tick(f.starts.at(-1)! + 15_000 - Date.now());
  const count = f.readSandbox.mock.calls.length;
  await tick(1500);
  expect(f.readSandbox).toHaveBeenCalledTimes(count + 1);
});
