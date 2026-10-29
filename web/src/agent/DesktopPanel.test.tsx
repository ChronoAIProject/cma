import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { DesktopPanel } from './DesktopPanel';

type Connection = EventTarget & {
  viewOnly: boolean;
  resizeSession: boolean;
  disconnect: () => void;
  url: string;
};
const { connections } = vi.hoisted(() => ({ connections: [] as Connection[] }));
vi.mock('@novnc/novnc', () => ({
  default: class extends EventTarget {
    scaleViewport = false;
    resizeSession = false;
    viewOnly = false;
    disconnect = vi.fn();
    constructor(
      _target: HTMLElement,
      readonly url: string,
    ) {
      super();
      connections.push(this);
    }
  },
}));
beforeEach(() => {
  connections.length = 0;
});

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
  expect(screen.getByRole('status')).toHaveTextContent('Disconnected');
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
