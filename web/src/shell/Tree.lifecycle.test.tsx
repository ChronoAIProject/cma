import '../test/connectedAccounts';
import { useState } from 'react';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import { CmaClientProvider } from '../api/context';
import type { SandboxDetail } from '../api/types';
import { AgentSurface } from '../agent/AgentSurface';
import { useAgentController } from '../agent/controller';
import type { AppRoute } from '../routing/routes';
import { agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { Tree } from './Tree';
import type { TreeProps } from './Tree';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each(['minimum', 'maximum', 'changing'] as const)(
  'opens retained history during pause and preserves its route and draft through resume without sending early with %s jitter',
  async (jitter) => {
    vi.useFakeTimers();
    const random = vi.spyOn(Math, 'random').mockReturnValue(jitter === 'minimum' ? 0 : 0.999);
    if (jitter === 'changing')
      random.mockReturnValueOnce(0).mockReturnValueOnce(0).mockReturnValueOnce(0);
    const transition = (
      status: 'pausing' | 'reviving' | 'active',
      purpose: 'pause' | 'revive',
    ): SandboxDetail => ({
      ...sandboxFixture(status),
      lifecycle: {
        pause: { allowed: false, reason: 'work_in_progress' },
        revive: { allowed: false, reason: 'work_in_progress' },
        operation: { purpose, phase: 'publish_active', startedAt: '2026-10-14T10:09:15Z' },
      },
    });
    let sandbox = transition('pausing', 'pause');
    const runAgent = vi.fn().mockResolvedValue(undefined);
    const readSandbox = vi.fn(async () => sandbox);
    const client = agentClient({ readSandbox, runAgent });
    const readHistory = vi.spyOn(client, 'readAgentTranscript');
    const observeNext = async () => {
      const previous = readSandbox.mock.calls.length;
      for (
        let elapsed = 0;
        elapsed < 5500 && readSandbox.mock.calls.length === previous;
        elapsed += 100
      )
        await act(() => vi.advanceTimersByTimeAsync(100));
      expect(readSandbox.mock.calls.length).toBeGreaterThan(previous);
    };
    function App({ current }: { readonly current: SandboxDetail }) {
      const [route, navigate] = useState<AppRoute>({ kind: 'home' });
      const controller = useAgentController(
        client,
        route.kind === 'agent' ? route.agentId : undefined,
      );
      return (
        <>
          <Tree
            viewerId="lifecycle-viewer"
            items={[{ sandbox: current, workspaces: [workspaceFixture()] }]}
            route={route}
            permissions={{
              createWorkspace: false,
              createAgent: false,
              pauseSandbox: false,
              deleteSandbox: false,
              removeWorkspace: false,
              stopAgent: false,
              deleteAgent: false,
              exportHistory: false,
            }}
            onNavigate={navigate}
            onAction={vi.fn()}
          />
          {route.kind === 'agent' ? (
            <AgentSurface
              agentId={route.agentId}
              controller={controller}
              detailsOpen={false}
              onDetailsOpenChange={vi.fn()}
              onNavigate={navigate}
            />
          ) : null}
        </>
      );
    }
    const draw = () => (
      <CmaClientProvider client={client}>
        <App current={sandbox} />
      </CmaClientProvider>
    );
    const view = render(draw());
    const link = screen.getByRole('link', { name: /Agent one/ });
    expect(link).toHaveAttribute('href', '/agents/agt_1');
    expect(link).toHaveTextContent('Pausing');
    fireEvent.click(link);
    await act(() => vi.advanceTimersByTimeAsync(0));
    const draft = screen.getByRole('textbox', { name: 'Message' });
    fireEvent.change(draft, { target: { value: 'Keep this draft until resume finishes' } });

    for (const next of [
      sandbox,
      sandboxFixture('paused'),
      transition('reviving', 'revive'),
      transition('active', 'revive'),
    ]) {
      // Exercise unchanged observations up to the maximum backoff before the
      // next phase, then require its read within the scheduler's jitter bound.
      await act(() => vi.advanceTimersByTimeAsync(15000));
      sandbox = next;
      view.rerender(draw());
      await observeNext();
      expect(screen.getByRole('link', { name: /Agent one/ })).toHaveAttribute(
        'aria-current',
        'page',
      );
      expect(screen.getAllByText('The repository is ready.')).toHaveLength(1);
      expect(draft).toHaveValue('Keep this draft until resume finishes');
      expect(screen.queryByRole('region', { name: 'Sandbox progress' })).not.toBeInTheDocument();
      if (sandbox.lifecycle?.operation) {
        expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
        fireEvent.keyDown(draft, { key: 'Enter' });
        fireEvent.submit(screen.getByRole('form', { name: 'Message Agent' }));
        expect(runAgent).not.toHaveBeenCalled();
      }
    }
    sandbox = sandboxFixture('active');
    view.rerender(draw());
    await observeNext();
    expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
    expect(readHistory).toHaveBeenCalledOnce();
    expect(runAgent).not.toHaveBeenCalled();
  },
);

it.each(['sandbox', 'agent'] as const)(
  'explains terminal Stop in the %s tree through Resume without dispatching',
  async (view) => {
    const user = userEvent.setup();
    const onAction = vi.fn();
    const props: TreeProps = {
      viewerId: 'stop-reasons',
      view,
      items: [],
      route: { kind: 'home' },
      permissions: {
        createWorkspace: false,
        createAgent: false,
        pauseSandbox: false,
        deleteSandbox: false,
        removeWorkspace: false,
        stopAgent: true,
        deleteAgent: true,
        exportHistory: true,
      },
      onNavigate: vi.fn(),
      onAction,
    };
    const rendered = render(<Tree {...props} />);
    for (const [status, reason] of [
      ['stopped', 'Agent is already stopped.'],
      ['failed', 'Agent has failed. There is no running Agent to stop.'],
    ] as const) {
      for (const sandboxStatus of ['paused', 'reviving', 'active'] as const) {
        rendered.rerender(
          <Tree
            {...props}
            items={[
              { sandbox: sandboxFixture(sandboxStatus), workspaces: [workspaceFixture(status)] },
            ]}
          />,
        );
        await user.click(screen.getByRole('button', { name: 'Options for Agent one' }));
        const stop = screen.getByRole('menuitem', { name: `Stop ${reason}` });
        expect(stop).toHaveAttribute('aria-disabled', 'true');
        expect(screen.getByRole('menuitem', { name: /^Delete/ })).toHaveAttribute(
          'aria-disabled',
          String(sandboxStatus !== 'active'),
        );
        expect(screen.getByRole('menuitem', { name: 'Export history' })).not.toHaveAttribute(
          'aria-disabled',
          'true',
        );
        await user.click(stop);
        act(() => stop.focus());
        await user.keyboard('{Enter} ');
        expect(onAction).not.toHaveBeenCalled();
        await user.keyboard('{Escape}');
      }
    }
    for (const { sandboxStatus, refreshError, lifecycleBusy, reason } of [
      { sandboxStatus: 'paused', reason: /resume the Sandbox/ },
      { sandboxStatus: 'reviving', reason: /Resuming\./ },
      {
        sandboxStatus: 'paused',
        refreshError: 'Refresh failed',
        reason: /unavailable.*Check its status/,
      },
      { sandboxStatus: 'active', lifecycleBusy: true, reason: /action is in progress/ },
    ] as const) {
      rendered.rerender(
        <Tree
          {...props}
          refreshError={refreshError}
          lifecycleBusy={lifecycleBusy}
          items={[{ sandbox: sandboxFixture(sandboxStatus), workspaces: [workspaceFixture()] }]}
        />,
      );
      await user.click(screen.getByRole('button', { name: 'Options for Agent one' }));
      const stop = screen.getByRole('menuitem', { name: /^Stop / });
      expect(stop).toHaveTextContent(reason);
      expect(stop).toHaveAttribute('aria-disabled', 'true');
      await user.keyboard('{Escape}');
    }
  },
);
