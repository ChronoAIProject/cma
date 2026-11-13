import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, createClient } from '@chronoai/cma-sdk';
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
