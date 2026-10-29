import { test, expect, type Page } from '@playwright/test';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';

/** Local RFB handshake only: proves bundled viewer/layout, not a remote Linux runtime. */
async function desktopBoundary(page: Page) {
  const connections: number[] = [];
  await page.routeWebSocket('**/api/v1/agents/agt_contract/desktop/ws', (socket) => {
    connections.push(connections.length);
    let step = 0;
    socket.onMessage(() => {
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
      }
      step++;
    });
    socket.send(Buffer.from('RFB 003.008\n'));
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
  return connections;
}

for (const width of [1600, 1024, 390]) {
  for (const theme of ['light', 'dark']) {
    test(`LOCAL desktop and live conversation coexist at ${width}px in ${theme}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
      try {
        await page.setViewportSize({ width, height: 1000 });
        await fixture.route(page, baseURL!);
        const connections = await desktopBoundary(page);
        await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
          route.fulfill({
            json: {
              object: 'cma.agent.event_head',
              schema_version: 1,
              agent_id: 'agt_contract',
              incarnation: 'a'.repeat(64),
              through_cursor: cursor(fixture.items.length),
              active_turn: null,
            },
          }),
        );
        await page.goto('/agents/agt_contract');
        await page.evaluate(
          (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
          theme,
        );
        const message = page.getByRole('textbox', { name: 'Message', exact: true });
        const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
        await expect(message).toBeEnabled();
        await expect(desktop.getByRole('status')).toContainText('Connected');
        await expect(
          desktop.getByRole('button', { name: 'Control desktop', exact: true }),
        ).toBeEnabled();
        await message.fill('Open a terminal and run pwd where I can see it.');
        const field = await message.elementHandle();
        await expect
          .poll(() =>
            page.evaluate((sideBySide) => {
              const chat = document.querySelector('.agent-surface')!.getBoundingClientRect();
              const desktop = document.querySelector('.agent-desktop')!.getBoundingClientRect();
              const canvas = document
                .querySelector('.agent-desktop canvas')!
                .getBoundingClientRect();
              return (
                document.documentElement.scrollWidth <= innerWidth &&
                desktop.bottom <= innerHeight + 1 &&
                canvas.left >= desktop.left &&
                canvas.right <= desktop.right + 1 &&
                canvas.bottom <= desktop.bottom + 1 &&
                (sideBySide ? desktop.left >= chat.right - 1 : desktop.top >= chat.bottom - 1)
              );
            }, width === 1600),
          )
          .toBe(true);
        await page.screenshot({ path: info.outputPath(`split-${width}-${theme}.png`) });
        await desktop.getByRole('button', { name: 'Full screen', exact: true }).click();
        await expect(
          desktop.getByRole('button', { name: 'Exit full screen', exact: true }),
        ).toBeVisible();
        await desktop.getByRole('button', { name: 'Exit full screen', exact: true }).click();
        expect(connections).toHaveLength(1);
        expect(await message.evaluate((node, original) => node === original, field)).toBe(true);
        await page.getByRole('button', { name: 'Hide machine desktop', exact: true }).click();
        await expect(desktop).toHaveCount(0);
        await page.getByRole('button', { name: 'Open machine desktop', exact: true }).click();
        await expect(desktop.getByRole('status')).toContainText('Connected');
        expect(await message.evaluate((node, original) => node === original, field)).toBe(true);
        await expect(message).toHaveValue('Open a terminal and run pwd where I can see it.');
        await expect.poll(() => fixture.streams.size).toBeGreaterThan(0);
        fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
        fixture.live('text_delta', 'Opening the terminal on your desktop.');
        await expect(desktop.getByRole('status')).toContainText('Agent is working');
        await expect(
          desktop.getByRole('button', { name: 'Control desktop', exact: true }),
        ).toBeDisabled();
        await expect(
          page.getByText('Opening the terminal on your desktop.', { exact: true }),
        ).toBeVisible();
        expect(fixture.writes).toEqual([]);
        expect(fixture.boundary.writes).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });
  }
}
