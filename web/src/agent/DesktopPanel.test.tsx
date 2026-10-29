import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DesktopPanel } from './DesktopPanel';

type Connection = EventTarget & {
  viewOnly: boolean;
  resizeSession: boolean;
  disconnect: () => void;
  url: string;
  painted: boolean;
  focus: () => void;
  blur: () => void;
};
const { connections } = vi.hoisted(() => ({ connections: [] as Connection[] }));
vi.mock('@novnc/novnc', () => ({
  default: class extends EventTarget {
    scaleViewport = false;
    resizeSession = false;
    viewOnly = false;
    painted = true;
    focus = vi.fn();
    blur = vi.fn();
    sendKey = vi.fn();
    canvas = document.createElement('canvas');
    disconnect = vi.fn(() => this.canvas.remove());
    constructor(
      target: HTMLElement,
      readonly url: string,
    ) {
      super();
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
afterEach(() => vi.useRealTimers());

function connectionAt(index: number): Connection {
  const connection = connections[index];
  if (!connection) throw new Error(`Missing desktop connection ${index}`);
  return connection;
}

it('requires explicit user input control and pauses it during an Agent turn', async () => {
  const onClose = vi.fn();
  const view = render(<DesktopPanel agentId="agt_demo" running={false} onClose={onClose} />);
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  await waitFor(() => expect(connections).toHaveLength(1));
  const connection = connectionAt(0);
  expect(connection.viewOnly).toBe(true);
  expect(connection.resizeSession).toBe(false);
  act(() => connection.dispatchEvent(new Event('connect')));
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(connection.viewOnly).toBe(false);
  view.rerender(<DesktopPanel agentId="agt_demo" running onClose={onClose} />);
  expect(connection.viewOnly).toBe(true);
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeDisabled();
  expect(screen.getByRole('status')).toHaveTextContent('Agent is working');
  view.rerender(<DesktopPanel agentId="agt_demo" running={false} onClose={onClose} />);
  expect(connection.viewOnly).toBe(true);
  expect(screen.getByText('The Agent finished. You can take control now.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Control desktop' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Close machine desktop' }));
  expect(onClose).toHaveBeenCalledOnce();
  view.unmount();
  expect(connection.disconnect).toHaveBeenCalledOnce();
});

it('reconnects with input disabled and releases the previous stream', async () => {
  const view = render(<DesktopPanel agentId="agt/one" running={false} onClose={vi.fn()} />);
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
  const view = render(<DesktopPanel agentId="agt_demo" running={false} onClose={vi.fn()} />);
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
  const view = render(<DesktopPanel agentId="agt_demo" running={false} onClose={vi.fn()} />);
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
  act(() => vi.advanceTimersByTime(200));
  expect(view.container.querySelector('.agent-desktop__loading')).not.toBeInTheDocument();
  view.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('shows each handoff for five seconds, replaces stale notices, and retains the control status', async () => {
  const view = render(<DesktopPanel agentId="agt_demo" running={false} onClose={vi.fn()} />);
  await waitFor(() => expect(connections).toHaveLength(1));
  vi.useFakeTimers();
  act(() => connectionAt(0).dispatchEvent(new Event('connect')));
  fireEvent.click(screen.getByRole('button', { name: 'Control desktop' }));
  expect(connectionAt(0).focus).toHaveBeenCalledOnce();
  expect(screen.getByText('You have control. Click and type on the desktop.')).toBeInTheDocument();
  act(() => vi.advanceTimersByTime(4_000));
  view.rerender(<DesktopPanel agentId="agt_demo" running onClose={vi.fn()} />);
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
  const view = render(<DesktopPanel agentId="agt_demo" running={false} onClose={vi.fn()} />);
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
