import { vi } from 'vitest';

export class DesktopSocket extends EventTarget {
  static OPEN = 1;
  static instances: DesktopSocket[] = [];
  readyState = 0;
  binaryType = 'blob';
  onmessage: WebSocket['onmessage'] = null;
  onerror: WebSocket['onerror'] = null;
  onclose: WebSocket['onclose'] = null;
  send = vi.fn();
  close = vi.fn(() => {
    this.readyState = 3;
  });

  constructor(readonly url: string) {
    super();
    DesktopSocket.instances.push(this);
  }

  receive(data: unknown) {
    this.onmessage?.call(this as unknown as WebSocket, new MessageEvent('message', { data }));
  }

  fail() {
    this.onerror?.call(this as unknown as WebSocket, new Event('error'));
  }
}

export function stubDesktopSockets() {
  DesktopSocket.instances.length = 0;
  vi.stubGlobal('WebSocket', DesktopSocket);
}

export function desktopSocket(index = 0) {
  const socket = DesktopSocket.instances[index];
  if (!socket) throw new Error(`Missing desktop socket ${index}`);
  return socket;
}
