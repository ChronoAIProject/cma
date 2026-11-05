import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DesktopPanel } from './DesktopPanel';
import { prepareDesktop } from './desktopPreparation';
import { desktopSocket, stubDesktopSockets } from './desktopPreparation.fixture';

type Connection = EventTarget & {
  viewOnly: boolean;
  resizeSession: boolean;
  disconnect: () => void;
  url: string | WebSocket;
  messages: unknown[];
  painted: boolean;
  focus: () => void;
  blur: () => void;
  canvas: HTMLCanvasElement;
};
const { connections } = vi.hoisted(() => ({ connections: [] as Connection[] }));
vi.mock('@novnc/novnc', () => ({
  default: class extends EventTarget {
    scaleViewport = false;
    resizeSession = false;
    viewOnly = false;
    painted = true;
    messages: unknown[] = [];
    focus = vi.fn();
    blur = vi.fn();
    sendKey = vi.fn();
    canvas = document.createElement('canvas');
    disconnect = vi.fn(() => this.canvas.remove());
    constructor(
      target: HTMLElement,
      readonly url: string | WebSocket,
    ) {
      super();
      if (typeof url !== 'string') url.onmessage = (event) => this.messages.push(event.data);
      Object.defineProperty(this.canvas, 'getContext', {
        value: () => ({
          getImageData: () => ({ data: new Uint8ClampedArray([0, 0, 0, this.painted ? 255 : 0]) }),
        }),
      });
      target.append(this.canvas);
      connections.push(this);
    }
  },
}));
beforeEach(() => {
  connections.length = 0;
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it('attaches the prepared stream once with input disabled, then reconnects normally', async () => {
  stubDesktopSockets();
  const preparation = { context: {}, owner: 'alice' };
  prepareDesktop(preparation.context, 'agt_demo', preparation.owner)?.handoff();
  const socket = desktopSocket();
  const greeting = new Uint8Array([82, 70, 66]).buffer;
  socket.receive(greeting);
  const view = render(
    <DesktopPanel
      runtimeReady
      agentId="agt_demo"
      running={false}
      onClose={vi.fn()}
      preparation={preparation}
    />,
  );
  await waitFor(() => expect(connections).toHaveLength(1));
  const first = connectionAt(0);
  expect(first.url).toBe(socket);
  expect(first.messages).toEqual([greeting]);
  expect(first.viewOnly).toBe(true);
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  act(() => first.dispatchEvent(new Event('connect')));
  expect(first.viewOnly).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(first.viewOnly).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
  await waitFor(() => expect(connections).toHaveLength(2));
  expect(first.disconnect).toHaveBeenCalledOnce();
  expect(connectionAt(1).url).toContain('/agents/agt_demo/desktop/ws');
  expect(connectionAt(1).viewOnly).toBe(true);
  view.unmount();
});

function connectionAt(index: number): Connection {
  const connection = connections[index];
  if (!connection) throw new Error(`Missing desktop connection ${index}`);
  return connection;
}

it('waits through startup and reconnects after revival without retaining input control', async () => {
  vi.useFakeTimers();
  const draw = (runtimeReady: boolean) => (
    <DesktopPanel
      agentId="agt_starting"
      runtimeReady={runtimeReady}
      running={false}
      onClose={vi.fn()}
    />
  );
  const view = render(draw(false));
  await act(async () => vi.advanceTimersByTime(60_000));
  expect(connections).toHaveLength(0);
  expect(screen.getByRole('status')).toHaveTextContent('Waiting for your machine');
  expect(screen.getByRole('button', { name: 'Reconnect' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  await act(async () => view.rerender(draw(true)));
  const first = connectionAt(0);
  act(() => first.dispatchEvent(new Event('connect')));
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(first.viewOnly).toBe(false);
  view.rerender(draw(false));
  expect(first.viewOnly).toBe(true);
  expect(first.disconnect).toHaveBeenCalledOnce();
  await act(async () => vi.advanceTimersByTime(60_000));
  expect(connections).toHaveLength(1);
  await act(async () => view.rerender(draw(true)));
  const revived = connectionAt(1);
  act(() => revived.dispatchEvent(new Event('connect')));
  act(() => first.dispatchEvent(new Event('disconnect')));
  expect(screen.getByRole('status')).toHaveTextContent('Connected');
  expect(revived.viewOnly).toBe(true);
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('requires explicit control and returns input to the running Agent on request', async () => {
  const onClose = vi.fn();
  const view = render(
    <DesktopPanel runtimeReady agentId="agt_demo" running={false} onClose={onClose} />,
  );
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  await waitFor(() => expect(connections).toHaveLength(1));
  const connection = connectionAt(0);
  expect(connection.viewOnly).toBe(true);
  expect(connection.resizeSession).toBe(false);
  act(() => connection.dispatchEvent(new Event('connect')));
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(connection.viewOnly).toBe(false);
  view.rerender(<DesktopPanel runtimeReady agentId="agt_demo" running onClose={onClose} />);
  expect(connection.viewOnly).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(connection.viewOnly).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'Return to Agent' }));
  expect(connection.viewOnly).toBe(true);
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeEnabled();
  expect(screen.getByRole('status')).toHaveTextContent('Agent is working');
  view.rerender(<DesktopPanel runtimeReady agentId="agt_demo" running={false} onClose={onClose} />);
  expect(connection.viewOnly).toBe(true);
  expect(screen.getByText('The Agent is paused. The desktop is idle.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Close machine desktop' }));
  expect(onClose).toHaveBeenCalledOnce();
  view.unmount();
  expect(connection.disconnect).toHaveBeenCalledOnce();
});

it('reconnects with input disabled and releases the previous stream', async () => {
  const view = render(
    <DesktopPanel runtimeReady agentId="agt/one" running={false} onClose={vi.fn()} />,
  );
  await waitFor(() => expect(connections).toHaveLength(1));
  const previous = connectionAt(0);
  expect(previous.url).toContain('/agents/agt%2Fone/desktop/ws');
  act(() => previous.dispatchEvent(new Event('connect')));
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(previous.viewOnly).toBe(false);
  act(() => previous.dispatchEvent(new Event('disconnect')));
  expect(screen.getByRole('status')).toHaveTextContent('Connection lost');
  fireEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
  await waitFor(() => expect(connections).toHaveLength(2));
  const current = connectionAt(1);
  expect(previous.disconnect).toHaveBeenCalledOnce();
  expect(current.viewOnly).toBe(true);
  act(() => current.dispatchEvent(new Event('connect')));
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeEnabled();
  view.unmount();
  expect(current.disconnect).toHaveBeenCalledOnce();
});

it('keeps the live viewer and controls mounted when entering and leaving full screen', async () => {
  const view = render(
    <DesktopPanel runtimeReady agentId="agt_demo" running={false} onClose={vi.fn()} />,
  );
  await waitFor(() => expect(connections).toHaveLength(1));
  const panel = screen.getByRole('region', { name: 'Machine desktop' });
  const request = vi.fn(async () => {
    Object.defineProperty(document, 'fullscreenElement', { configurable: true, value: panel });
    document.dispatchEvent(new Event('fullscreenchange'));
  });
  const exit = vi.fn(async () => {
    Object.defineProperty(document, 'fullscreenElement', { configurable: true, value: null });
    document.dispatchEvent(new Event('fullscreenchange'));
  });
  Object.defineProperty(panel, 'requestFullscreen', { configurable: true, value: request });
  Object.defineProperty(document, 'exitFullscreen', { configurable: true, value: exit });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Full screen' })));
  expect(request).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeInTheDocument();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Exit full screen' })));
  expect(exit).toHaveBeenCalledOnce();
  expect(connections).toHaveLength(1);
  expect(connectionAt(0).disconnect).not.toHaveBeenCalled();
  view.unmount();
});

it('keeps loading until the first painted screen, including a real black desktop', async () => {
  const view = render(
    <DesktopPanel runtimeReady agentId="agt_demo" running={false} onClose={vi.fn()} />,
  );
  await waitFor(() => expect(connections).toHaveLength(1));
  vi.useFakeTimers();
  const connection = connectionAt(0);
  connection.painted = false;
  act(() => connection.dispatchEvent(new Event('connect')));
  expect(screen.getByRole('status')).toHaveTextContent('Loading the desktop screen');
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  act(() => vi.advanceTimersByTime(1_000));
  expect(view.container.querySelector('.agent-desktop__loading')).toBeInTheDocument();
  connection.painted = true;
  act(() => vi.advanceTimersByTime(50));
  expect(screen.getByRole('status')).toHaveTextContent('Connected');
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeEnabled();
  expect(view.container.querySelector('.agent-desktop__loading')).not.toBeInTheDocument();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('shows each handoff for five seconds, replaces stale notices, and retains the control status', async () => {
  const view = render(
    <DesktopPanel runtimeReady agentId="agt_demo" running={false} onClose={vi.fn()} />,
  );
  await waitFor(() => expect(connections).toHaveLength(1));
  vi.useFakeTimers();
  act(() => connectionAt(0).dispatchEvent(new Event('connect')));
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(connectionAt(0).focus).toHaveBeenCalledOnce();
  expect(screen.getByText('You have control. Click and type on the desktop.')).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(4_000));
  view.rerender(<DesktopPanel runtimeReady agentId="agt_demo" running onClose={vi.fn()} />);
  expect(
    screen.queryByText('You have control. Click and type on the desktop.'),
  ).not.toBeInTheDocument();
  expect(screen.getByText('The Agent has control. Watch its actions here.')).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(4_999));
  expect(view.container.querySelector('.agent-desktop__notice')).toHaveAttribute(
    'data-presence',
    'visible',
  );
  act(() => vi.advanceTimersByTime(1));
  expect(view.container.querySelector('.agent-desktop__notice')).toHaveAttribute('inert');
  act(() => vi.advanceTimersByTime(200));
  expect(view.container.querySelector('.agent-desktop__notice')).not.toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Agent is working');
  view.unmount();
});

it('offers recovery after a missing first frame and cancels the old loading timers', async () => {
  vi.useFakeTimers();
  const view = render(
    <DesktopPanel runtimeReady agentId="agt_demo" running={false} onClose={vi.fn()} />,
  );
  await act(async () => {});
  const connection = connectionAt(0);
  connection.painted = false;
  act(() => connection.dispatchEvent(new Event('connect')));
  act(() => vi.advanceTimersByTime(30_000));
  expect(screen.getByRole('status')).toHaveTextContent('taking longer than expected');
  expect(connection.disconnect).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Reconnect' })));
  const next = connectionAt(1);
  act(() => next.dispatchEvent(new Event('connect')));
  expect(screen.getByRole('status')).toHaveTextContent('Connected');
  act(() => connection.dispatchEvent(new Event('disconnect')));
  expect(screen.getByRole('status')).toHaveTextContent('Connected');
  expect(next.viewOnly).toBe(true);
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('restores a lost display with input revoked and bounds automatic recovery', async () => {
  vi.useFakeTimers();
  const view = render(
    <DesktopPanel runtimeReady agentId="agt_demo" running={false} onClose={vi.fn()} />,
  );
  await act(async () => {});
  const first = connectionAt(0);
  act(() => first.dispatchEvent(new Event('connect')));
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(first.viewOnly).toBe(false);
  act(() => first.canvas.dispatchEvent(new Event('contextlost')));
  expect(first.viewOnly).toBe(true);
  expect(screen.getByRole('status')).toHaveTextContent('Restoring the desktop display');
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  await act(async () => vi.advanceTimersByTime(750));
  const second = connectionAt(1);
  act(() => second.dispatchEvent(new Event('connect')));
  expect(screen.getByRole('status')).toHaveTextContent('Connected');
  expect(second.viewOnly).toBe(true);
  act(() => first.dispatchEvent(new Event('disconnect')));
  expect(screen.getByRole('status')).toHaveTextContent('Connected');
  act(() => second.canvas.dispatchEvent(new Event('contextlost')));
  await act(async () => vi.advanceTimersByTime(750));
  const third = connectionAt(2);
  act(() => third.dispatchEvent(new Event('connect')));
  act(() => third.canvas.dispatchEvent(new Event('contextlost')));
  expect(screen.getByRole('status')).toHaveTextContent('could not be restored');
  await act(async () => vi.advanceTimersByTime(30_000));
  expect(connections).toHaveLength(3);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Reconnect' })));
  const manual = connectionAt(3);
  act(() => manual.dispatchEvent(new Event('connect')));
  act(() => manual.canvas.dispatchEvent(new Event('contextlost')));
  expect(screen.getByRole('status')).toHaveTextContent('Restoring the desktop display');
  view.unmount();
  await act(async () => vi.advanceTimersByTime(30_000));
  expect(connections).toHaveLength(4);
  expect(vi.getTimerCount()).toBe(0);
});
