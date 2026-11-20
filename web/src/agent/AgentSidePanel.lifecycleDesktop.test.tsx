import { createClient } from '@cma/client';
import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { computerFixture, live, paused } from './computerLifecycle.fixture';

const { connect } = vi.hoisted(() => ({ connect: vi.fn() }));
vi.mock('@novnc/novnc', () => ({ default: connect }));
afterEach(() => vi.restoreAllMocks());

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
