import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxEvent } from '../api/types';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { agentProgress, sandboxProgress, workspaceProgress } from './progress';
import { SandboxPage } from './SandboxPage';
import { readSandboxHistory } from './eventHistory';
import { ProvisioningProgress } from './ProvisioningProgress';

afterEach(() => vi.useRealTimers());
function event(seq: number, kind: SandboxEvent['kind']): SandboxEvent {
  return { sandboxId: 'sbx_1', seq, appendedAt: '2026-09-06T00:00:00Z', kind };
}

it('advances only recorded Sandbox steps and ignores another Sandbox history', () => {
  const sandbox = sandboxFixture('provisioning');
  expect(sandboxProgress(sandbox, []).map((step) => step.status)).toEqual([
    'In progress',
    'Waiting',
    'Waiting',
    'Waiting',
    'Waiting',
  ]);
  const events = ['provision_runtime', 'run_install', 'deliver_initial_credential'].map(
    (kind, index) => event(index + 1, { type: 'work_step_completed', step: { kind } }),
  );
  expect(sandboxProgress(sandbox, events).map((step) => step.status)).toEqual([
    'Done',
    'Done',
    'Done',
    'In progress',
    'Waiting',
  ]);
  expect(
    sandboxProgress(
      sandbox,
      events.map((item) => ({ ...item, sandboxId: 'foreign' })),
    )[0]?.status,
  ).toBe('In progress');
  expect(sandboxProgress(sandboxFixture(), events).every((step) => step.status === 'Done')).toBe(
    true,
  );
});

it('attributes a failure only to the recorded work step', () => {
  const failure = { class: 'install_failed', detail: 'Software installation did not complete.' };
  const steps = sandboxProgress({ ...sandboxFixture('failed'), failure }, [
    event(1, { type: 'work_step_completed', step: { kind: 'provision_runtime' } }),
    event(2, { type: 'work_step_failed', step: { kind: 'run_install' }, failure }),
  ]);
  expect(steps[1]).toMatchObject({
    label: 'Installing software',
    status: "Didn't work",
    detail: failure.detail,
  });
});

it('renders server-provided remediation and keeps a fallback for older failure records', () => {
  const failure = {
    class: 'install_failed',
    detail: 'Software installation did not complete.',
    remediation: 'Review the install commands before creating a replacement Sandbox.',
  };
  const view = render(
    <ProvisioningProgress title="Sandbox preparation" steps={[]} failure={failure} />,
  );
  expect(screen.getByRole('alert')).toHaveTextContent(failure.remediation);
  view.rerender(
    <ProvisioningProgress
      title="Sandbox preparation"
      steps={[]}
      failure={{ class: failure.class, detail: failure.detail }}
    />,
  );
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Review the selected profile before creating a replacement.',
  );
});

it('keeps unreported Workspace setup waiting until preparation is evidenced', () => {
  const workspace = {
    ...workspaceFixture(),
    status: 'preparing' as const,
    resolvedCommit: undefined,
  };
  expect(workspaceProgress(workspace, []).map((step) => step.status)).toEqual([
    'In progress',
    'Waiting',
    'Waiting',
  ]);
  expect(workspaceProgress({ ...workspace, resolvedCommit: 'commit' }, [])[1]?.status).toBe(
    'In progress',
  );
  expect(
    workspaceProgress(workspace, [
      event(1, { type: 'workspace_ready', workspace_id: 'wsp_1' }),
    ]).every((step) => step.status === 'Done'),
  ).toBe(true);
  expect(workspaceProgress({ ...workspace, source: { type: 'empty' } }, [])[0]?.label).toBe(
    'Creating directory',
  );
});

it('uses the Agent worktree and durable readiness without claiming an uncreated worktree is ready', () => {
  const agent = {
    ...AGENT_DETAIL,
    status: 'starting' as const,
    readyAt: undefined,
    worktree: { ...AGENT_DETAIL.worktree, status: 'creating' as const },
  };
  expect(agentProgress(agent, []).map((step) => step.status)).toEqual([
    'In progress',
    'Waiting',
    'Waiting',
  ]);
  expect(
    agentProgress({ ...agent, worktree: { ...agent.worktree, status: 'ready' } }, [])[1]?.status,
  ).toBe('In progress');
  expect(
    agentProgress(agent, [
      event(1, { type: 'agent_status_changed', agent_id: 'agt_1', to: 'ready' }),
    ]).every((step) => step.status === 'Done'),
  ).toBe(true);
});

it('polls incremental Sandbox events, preserves progress on read failure, and slows after readiness', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce({ ...sandboxFixture('provisioning'), workspaces: [] })
    .mockRejectedValueOnce(new Error('offline'))
    .mockResolvedValue({ ...sandboxFixture(), workspaces: [] });
  const readSandboxEvents = vi.fn(async (_id, options) =>
    options.cursor === 0 ? [event(1, { type: 'created' })] : [],
  );
  const view = render(
    <SandboxPage
      client={agentClient({ readSandbox, readSandboxEvents })}
      sandboxId="sbx_1"
      revision={0}
      permissions={{
        createAgent: true,
        createWorkspace: true,
        pauseSandbox: true,
        deleteSandbox: true,
        removeWorkspace: true,
        deleteAgent: true,
        stopAgent: true,
        exportHistory: true,
      }}
      busy={false}
      onAction={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(readSandboxEvents.mock.calls.map((call) => call[1].cursor)).toEqual([0, 1]);
  expect(
    within(screen.getByRole('region', { name: 'Live activity' })).getByText('Sandbox created'),
  ).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
  await act(async () =>
    fireEvent.click(screen.getByRole('menuitem', { name: 'Technical details' })),
  );
  expect(
    within(screen.getByRole('region', { name: 'Sandbox preparation' })).getByText(
      'Starting the sandbox',
    ),
  ).toBeInTheDocument();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1500);
  });
  expect(screen.getByRole('alert')).toHaveTextContent('Progress could not be refreshed');
  expect(screen.getByRole('region', { name: 'Sandbox preparation' })).toBeInTheDocument();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(readSandboxEvents.mock.calls.at(-1)?.[1].cursor).toBe(1);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(10000);
  });
  expect(readSandbox).toHaveBeenCalledTimes(5);
  expect(readSandboxEvents.mock.calls.at(-1)?.[1].cursor).toBe(1);
  expect(screen.queryByRole('region', { name: 'Sandbox preparation' })).not.toBeInTheDocument();
  view.unmount();
});

it('rejects crossed or nonmonotonic events and respects cancellation', async () => {
  await expect(
    readSandboxHistory(
      agentClient({
        readSandboxEvents: async () => [
          event(2, { type: 'created' }),
          event(1, { type: 'created' }),
        ],
      }),
      'sbx_1',
      new AbortController().signal,
    ),
  ).rejects.toThrow('did not advance');
  const request = new AbortController();
  request.abort();
  const readSandboxEvents = vi.fn();
  await expect(
    readSandboxHistory(agentClient({ readSandboxEvents }), 'sbx_1', request.signal),
  ).rejects.toThrow();
  expect(readSandboxEvents).not.toHaveBeenCalled();
});
