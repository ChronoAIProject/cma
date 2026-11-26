import type { Page } from '@playwright/test';

/** Local RFB frames prove viewer behavior; remote-runtime evidence is separate. */
export async function desktopBoundary(
  page: Page,
  autoPaint = true,
  autoConnect: boolean | ((connection: number) => boolean) = true,
) {
  // Reconnect discovery is a public HTTP read before the RFB upgrade. Keep
  // the fixture boundary complete instead of falling through to a real API.
  await page.route('**/api/v1/agents/agt_contract/desktop', (route) =>
    route.fulfill({ json: { websocket_url: '/api/v1/agents/agt_contract/desktop/ws' } }),
  );
  const connections: number[] = [];
  const painters: (() => void)[] = [];
  const starters: (() => void)[] = [];
  const closers: (() => void)[] = [];
  const inputs: { kind: 'key' | 'pointer'; down?: boolean; keysym?: number; buttons?: number }[] =
    [];
  await page.routeWebSocket('**/api/v1/agents/agt_contract/desktop/ws', (socket) => {
    const index = connections.length;
    connections.push(index);
    closers.push(() => socket.close());
    const paint = () => {
      // One actual, opaque black RRE framebuffer rectangle. A handshake alone
      // must not dismiss the loading surface or enable input.
      const update = Buffer.alloc(24);
      update.writeUInt16BE(1, 2);
      update.writeUInt16BE(1280, 8);
      update.writeUInt16BE(800, 10);
      update.writeInt32BE(2, 12);
      socket.send(update);
    };
    painters.push(paint);
    let step = 0;
    socket.onMessage((message) => {
      const bytes = Buffer.from(message);
      if (step > 2 && bytes[0] === 4 && bytes.length === 8)
        inputs.push({ kind: 'key', down: bytes[1] === 1, keysym: bytes.readUInt32BE(4) });
      if (step > 2 && bytes[0] === 5 && bytes.length === 6)
        inputs.push({ kind: 'pointer', buttons: bytes[1] });
      if (step === 0) socket.send(Buffer.from([1, 1]));
      if (step === 1) socket.send(Buffer.alloc(4));
      if (step === 2) {
        const name = Buffer.from('Local desktop fixture');
        const init = Buffer.alloc(24 + name.length);
        init.writeUInt16BE(1280, 0);
        init.writeUInt16BE(800, 2);
        init.set([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0], 4);
        init.writeUInt32BE(name.length, 20);
        name.copy(init, 24);
        socket.send(init);
        if (autoPaint) paint();
      }
      step++;
    });
    const start = () => socket.send(Buffer.from('RFB 003.008\n'));
    starters.push(start);
    if (typeof autoConnect === 'function' ? autoConnect(index) : autoConnect) start();
  });
  // Playwright adds a prototype level to WebSocket. noVNC checks only one level;
  // expose the routed socket's existing descriptors without replacing its behavior.
  await page.addInitScript(() => {
    window.WebSocket = new Proxy(window.WebSocket, {
      construct(target, args) {
        const socket = Reflect.construct(target, args) as WebSocket;
        for (const key of [
          'send',
          'close',
          'binaryType',
          'onerror',
          'onmessage',
          'onopen',
          'protocol',
          'readyState',
        ]) {
          let prototype = Object.getPrototypeOf(socket) as object | null;
          while (prototype) {
            const descriptor = Object.getOwnPropertyDescriptor(prototype, key);
            if (descriptor) {
              Object.defineProperty(socket, key, { ...descriptor, enumerable: true });
              break;
            }
            prototype = Object.getPrototypeOf(prototype) as object | null;
          }
        }
        return socket;
      },
    });
  });
  return {
    connections,
    inputs,
    paint: () => painters.at(-1)?.(),
    connect: () => starters.at(-1)?.(),
    disconnect: () => closers.at(-1)?.(),
  };
}
