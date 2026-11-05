import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { CmaClientProvider } from './api/context';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { agentClient } from './test/agent';
import { metricsClient } from './usage/metrics/testFixtures';
import './test/connectedAccounts';

vi.mock('./usage/metrics/MetricChart', () => ({ MetricChart: () => <p>Measured chart</p> }));
afterEach(() => window.history.replaceState(null, '', '/'));
it.each([false, true])(
  'keeps the Agent editor and stream with measured visibility=%s across Usage navigation',
  async (measured) => {
    window.history.replaceState(null, '', '/agents/agt_1');
    const actor = userEvent.setup();
    const metrics = metricsClient(false);
    const replay = vi
      .fn<NonNullable<ReturnType<typeof agentClient>['replayAgent']>>()
      .mockImplementation(
        async (_id, options) =>
          new Promise<void>((resolve) =>
            options.signal?.addEventListener('abort', () => resolve()),
          ),
      );
    const run = vi.fn();
    const adminMetrics = metricsClient(true);
    const client = agentClient({
      replayAgent: replay,
      runAgent: run,
      usageViews: {
        readMeasuredUsage: vi.fn().mockResolvedValue(measured),
        usageMetrics: metrics,
        usage: { events: vi.fn().mockResolvedValue({ items: [], next: null }) },
        agents: { list: vi.fn() },
      },
    });
    render(
      <AuthContextProvider
        provider={createMockAuthProvider({
          initiallyAuthenticated: true,
          capabilities: ['use_own_sandboxes', 'read_all_sandboxes'],
        })}
      >
        <CmaClientProvider client={{ ...client, ...{ adminUsageMetrics: adminMetrics } }}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>,
    );
    const editor = await screen.findByRole('textbox', { name: 'Message' });
    fireEvent.change(editor, { target: { value: 'Preserve this unsent draft' } });
    const trigger = screen.getByRole('button', { name: /Account menu for/ });
    await actor.click(trigger);
    await actor.click(screen.getByRole('menuitem', { name: 'Usage' }));
    const dialog = await screen.findByRole('dialog', { name: 'Your usage' });
    expect(window.location.pathname).toBe('/usage');
    const saved = window.history.state;
    await within(dialog).findByRole('button', { name: 'Research Sandbox' });
    expect(metrics.create).toHaveBeenCalledTimes(1);
    expect(adminMetrics.create).not.toHaveBeenCalled();
    if (measured)
      await actor.click(await within(dialog).findByRole('button', { name: 'Memory working set' }));
    else {
      expect(
        within(dialog).queryByRole('button', { name: 'Memory working set' }),
      ).not.toBeInTheDocument();
      expect(
        within(dialog).getByRole('region', { name: 'Allocated capacity now' }),
      ).toBeInTheDocument();
      expect(metrics.series).not.toHaveBeenCalled();
    }
    expect(metrics.create).toHaveBeenCalledTimes(1);
    expect(editor).toBeInTheDocument();
    await actor.keyboard('{Escape}');
    await waitFor(() => expect(window.location.pathname).toBe('/agents/agt_1'));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(editor);
    expect(editor).toHaveValue('Preserve this unsent draft');
    expect(trigger).toHaveFocus();
    expect(replay).toHaveBeenCalledTimes(1);
    expect(replay.mock.calls[0]![1].signal?.aborted).toBe(false);
    expect(run).not.toHaveBeenCalled();
    act(() => {
      window.history.replaceState(saved, '', '/usage');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    await screen.findByRole('dialog', { name: 'Your usage' });
    expect(editor).toHaveValue('Preserve this unsent draft');
    expect(replay).toHaveBeenCalledTimes(1);
  },
);
