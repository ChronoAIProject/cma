import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { agentClient, AGENT_DETAIL } from '../test/agent';
import { ArtifactPublicationScope } from '../resources/artifactRecovery';
import { conversationFromTranscript } from './conversation';
import { AgentSidePanel } from './AgentSidePanel';
import { coalesceDetailReads } from '../api/detailReads';
import { prepareDesktop } from './desktopPreparation';
import { stubDesktopSockets, desktopSocket } from './desktopPreparation.fixture';

const { connections } = vi.hoisted(() => ({
  connections: [] as Array<
    EventTarget & { disconnect: ReturnType<typeof vi.fn>; viewOnly: boolean }
  >,
}));
vi.mock('@novnc/novnc', () => ({
  default: class extends EventTarget {
    viewOnly = true;
    canvas = document.createElement('canvas');
    disconnect = vi.fn(() => this.canvas.remove());
    focus = vi.fn();
    blur = vi.fn();
    sendKey = vi.fn();
    constructor(target: HTMLElement) {
      super();
      Object.defineProperty(this.canvas, 'getContext', {
        value: () => ({ getImageData: () => ({ data: [0, 0, 0, 255] }) }),
      });
      target.append(this.canvas);
      connections.push(this);
    }
  },
}));
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  connections.length = 0;
});

it('keeps a healthy desktop through detail/environment refresh and preserves a retry deadline through panel reconnect', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const sdk = createClient({ baseUrl: 'https://fixture.invalid' });
  const discover = vi
    .spyOn(sdk.agents, 'desktop')
    .mockResolvedValue({ websocket_url: '/api/v1/agents/agt_1/desktop/ws' });
  const environment = vi
    .fn()
    .mockRejectedValue(
      new ApiError(429, 'Busy', 'buffered_read_capacity_exhausted', undefined, 20_000),
    );
  const client = agentClient({ resourceViews: sdk, files: { environment } as never });
  const conversation = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
  const scope = new ArtifactPublicationScope();
  const draw = (loading = false) => (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        viewerId="owner"
        shared={false}
        panel="browser"
        detail={{ ...AGENT_DETAIL, desktop: true }}
        conversation={conversation}
        historyLoading={loading}
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={scope}
      />
    </CmaClientProvider>
  );
  const view = render(draw());
  await act(async () => {});
  expect(connections).toHaveLength(1);
  act(() => connections[0]!.dispatchEvent(new Event('connect')));
  view.rerender(draw(true));
  await act(() => vi.advanceTimersByTimeAsync(20_000));
  expect(connections).toHaveLength(1);
  expect(connections[0]!.disconnect).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: 'Take control' })).toBeDisabled();
  view.rerender(draw());
  expect(connections).toHaveLength(1);
  discover.mockRejectedValueOnce(new ApiError(409, 'Busy', 'desktop_busy', undefined, 20_000));
  act(() => connections[0]!.dispatchEvent(new Event('disconnect')));
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(discover).toHaveBeenCalledTimes(2);
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
  await act(() => vi.advanceTimersByTimeAsync(19_999));
  expect(discover).toHaveBeenCalledTimes(2);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(discover).toHaveBeenCalledTimes(3);
  expect(connections).toHaveLength(2);
  expect(connections[1]!.viewOnly).toBe(true);
  view.unmount();
});

it('consumes the owner creation socket through a scoped read wrapper without opening a rival stream', async () => {
  stubDesktopSockets();
  const sdk = createClient({ baseUrl: 'https://fixture.invalid' });
  const discover = vi.spyOn(sdk.agents, 'desktop');
  const source = agentClient({ resourceViews: sdk });
  prepareDesktop(source, 'agt_1', 'owner')?.handoff();
  const prepared = desktopSocket();
  const scope = coalesceDetailReads(source);
  const view = render(
    <CmaClientProvider client={scope.client}>
      <AgentSidePanel
        agentId="agt_1"
        viewerId="owner"
        shared={false}
        panel="browser"
        detail={{ ...AGENT_DETAIL, desktop: true }}
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={new ArtifactPublicationScope()}
      />
    </CmaClientProvider>,
  );
  await act(async () => {});
  expect(connections).toHaveLength(1);
  expect(discover).not.toHaveBeenCalled();
  expect(prepared.close).not.toHaveBeenCalled();
  view.unmount();
  scope.dispose();
});

it('retains desktop takeover through history loading and revokes it when the control baseline is lost', async () => {
  const sdk = createClient({ baseUrl: 'https://fixture.invalid' });
  vi.spyOn(sdk.agents, 'desktop').mockResolvedValue({
    websocket_url: '/api/v1/agents/agt_1/desktop/ws',
  });
  const client = agentClient({ resourceViews: sdk });
  const conversation = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
  const scope = new ArtifactPublicationScope();
  const draw = (controlReady = true, loading = true) => (
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        viewerId="owner"
        shared={false}
        panel="browser"
        detail={{ ...AGENT_DETAIL, desktop: true, status: 'working' }}
        conversation={conversation}
        controlReady={controlReady}
        historyLoading={loading}
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={scope}
      />
    </CmaClientProvider>
  );
  const view = render(draw());
  await act(async () => {});
  act(() => connections[0]!.dispatchEvent(new Event('connect')));
  expect(screen.getByRole('button', { name: 'Take control' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Take control' }));
  // An idle authoritative head overrides stale working metadata.
  expect(screen.getByRole('button', { name: 'Return to Agent' })).toBeEnabled();
  expect(connections[0]!.viewOnly).toBe(false);
  view.rerender(draw(true, false));
  view.rerender(draw(true, true));
  expect(screen.getByRole('button', { name: 'Return to Agent' })).toBeEnabled();
  expect(connections[0]!.viewOnly).toBe(false);
  view.rerender(draw(false));
  expect(screen.getByRole('button', { name: 'Take control' })).toBeDisabled();
  expect(connections[0]!.viewOnly).toBe(true);
  view.unmount();
});

it('restarts the exhausted embedded desktop only on explicit Reconnect and returns to view-only', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  const sdk = createClient({ baseUrl: 'https://fixture.invalid' });
  const discover = vi
    .spyOn(sdk.agents, 'desktop')
    .mockRejectedValue(new ApiError(503, 'Synthetic failure'));
  const client = agentClient({ resourceViews: sdk });
  const view = render(
    <CmaClientProvider client={client}>
      <AgentSidePanel
        agentId="agt_1"
        viewerId="owner"
        shared={false}
        panel="browser"
        detail={{ ...AGENT_DETAIL, desktop: true }}
        conversation={conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] })}
        onClose={vi.fn()}
        onAccessChanged={vi.fn()}
        publicationScope={new ArtifactPublicationScope()}
      />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(300_000));
  expect(discover).toHaveBeenCalledTimes(14);
  expect(screen.getByRole('region', { name: 'Machine desktop' })).toHaveTextContent(
    'Could not reconnect automatically.',
  );
  expect(screen.getByRole('button', { name: 'Take control' })).toBeDisabled();
  discover.mockResolvedValue({ websocket_url: '/api/v1/agents/agt_1/desktop/ws' });
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
  await act(() => vi.advanceTimersByTimeAsync(999));
  expect(discover).toHaveBeenCalledTimes(14);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(discover).toHaveBeenCalledTimes(15);
  act(() => connections[0]!.dispatchEvent(new Event('connect')));
  expect(screen.getByRole('button', { name: 'Take control' })).toBeEnabled();
  expect(connections[0]!.viewOnly).toBe(true);
  view.unmount();
});
