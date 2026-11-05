import { test, expect } from '@playwright/test';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';

import { desktopBoundary } from './desktop-rfb.fixture';

test('LOCAL a starting Machine connects when ready without a page reload', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  const ready = { ...fixture.boundary.agent, desktop: true };
  fixture.boundary.agent = {
    ...ready,
    status: 'starting',
    sandbox: { ...ready.sandbox, status: 'installing' },
  };
  try {
    await fixture.route(page, baseURL!);
    const stream = await desktopBoundary(page);
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
    const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
    await expect(desktop.getByRole('status')).toContainText('Waiting for your machine');
    await expect(page.getByRole('button', { name: 'Reconnect', exact: true })).toBeVisible();
    await expect(desktop.getByRole('button', { name: 'Take control', exact: true })).toBeDisabled();
    const surface = await page.locator('.agent-surface').elementHandle();
    expect(stream.connections).toHaveLength(0);
    fixture.boundary.agent = { ...ready, status: 'starting' };
    await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
    expect(stream.connections).toHaveLength(1);
    fixture.boundary.agent = ready;
    expect(
      await page.locator('.agent-surface').evaluate((node, original) => node === original, surface),
    ).toBe(true);
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.writes).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL restores a cleared desktop without losing the conversation or granting input', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
  try {
    // Pause before navigation so every application timer is controlled.
    const clockStart = Date.now();
    await page.clock.install({ time: clockStart - 60_000 });
    await page.clock.pauseAt(clockStart);
    await page.setViewportSize({ width: 1600, height: 1000 });
    await fixture.route(page, baseURL!);
    // Hold the external handshake until restoration/input safety is observed.
    const stream = await desktopBoundary(page, true, (connection) => connection === 0);
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
    const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
    const message = page.getByRole('textbox', { name: 'Message', exact: true });
    // Drive the first-frame timer while the browser clock remains paused.
    await expect
      .poll(async () => {
        await page.clock.runFor(50);
        return desktop.getByRole('status').textContent();
      })
      .toContain('Agent paused · Desktop is idle');
    await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
    await message.fill('Keep this unsent draft while restoring the display.');
    const originalComposer = await message.elementHandle();
    await desktop.getByRole('button', { name: 'Take control', exact: true }).click();
    const originalCanvas = await desktop.locator('canvas').elementHandle();
    // A real browser drawing-buffer reset leaves a transparent canvas while
    // the WebSocket remains open, matching the reported connected/blank state.
    await desktop.locator('canvas').evaluate((canvas: HTMLCanvasElement) => {
      canvas.getContext('2d')?.reset();
    });
    // Each jump fires the 2s display watchdog once. The second detects loss;
    // its 750ms reconnect timer stays paused while we assert input is revoked.
    await page.clock.fastForward(2_000);
    await page.clock.fastForward(2_000);
    await expect(desktop.getByRole('status')).toContainText('Restoring', { timeout: 6_000 });
    await expect(desktop.getByRole('button', { name: 'Take control', exact: true })).toBeDisabled();
    expect(stream.connections).toHaveLength(1);
    await page.clock.resume();
    await expect.poll(() => stream.connections.length).toBe(2);
    stream.connect();
    await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
    await expect(desktop.getByRole('button', { name: 'Take control', exact: true })).toBeEnabled();
    expect(
      await desktop
        .locator('canvas')
        .evaluate((node, previous) => node === previous, originalCanvas),
    ).toBe(false);
    expect(
      await desktop
        .locator('canvas')
        .evaluate(
          (node: HTMLCanvasElement) => node.getContext('2d')?.getImageData(0, 0, 1, 1).data[3],
        ),
    ).toBe(255);
    expect(await message.evaluate((node, previous) => node === previous, originalComposer)).toBe(
      true,
    );
    await expect(message).toHaveValue('Keep this unsent draft while restoring the display.');
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.writes).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

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
        const { connections } = await desktopBoundary(page);
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
        await message.fill('Open a terminal and run pwd where I can see it.');
        const field = await message.elementHandle();
        if (width < 1024) await page.getByRole('button', { name: 'Show Agent computer' }).click();
        await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
        await expect(
          desktop.getByRole('button', { name: 'Take control', exact: true }),
        ).toBeEnabled();
        const expectPaintedDesktop = async () => {
          await expect
            .poll(() =>
              desktop.locator('canvas').evaluate((canvas: HTMLCanvasElement) => {
                const bounds = canvas.getBoundingClientRect();
                const alpha = canvas.getContext('2d')?.getImageData(0, 0, 1, 1).data[3];
                return bounds.width > 100 && bounds.height > 60 && alpha === 255;
              }),
            )
            .toBe(true);
        };
        await expectPaintedDesktop();
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
                (sideBySide ? desktop.left >= chat.right - 1 : desktop.left >= 0)
              );
            }, width >= 1024),
          )
          .toBe(true);
        await page.screenshot({ path: info.outputPath(`split-${width}-${theme}.png`) });
        await page.getByRole('button', { name: 'Full screen', exact: true }).click();
        await expect(
          page.getByRole('button', { name: 'Exit full screen', exact: true }),
        ).toBeVisible();
        await page.getByRole('button', { name: 'Exit full screen', exact: true }).click();
        await expectPaintedDesktop();
        expect(connections).toHaveLength(1);
        expect(await message.evaluate((node, original) => node === original, field)).toBe(true);
        await page.getByRole('button', { name: 'Close computer', exact: true }).click();
        await expect(desktop).toHaveCount(0);
        await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
        await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
        expect(await message.evaluate((node, original) => node === original, field)).toBe(true);
        await expect(message).toHaveValue('Open a terminal and run pwd where I can see it.');
        await expect.poll(() => fixture.streams.size).toBeGreaterThan(0);
        fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
        fixture.live('text_delta', 'Opening the terminal on your desktop.');
        await expect(desktop.getByRole('status')).toContainText(
          'Agent is in control · Working in Desktop',
        );
        await expect(
          desktop.getByRole('button', { name: 'Take control', exact: true }),
        ).toBeEnabled();
        await expect(
          page.getByText('Opening the terminal on your desktop.', { exact: true }),
        ).toBeVisible();
        fixture.append({
          type: 'tool_started',
          call_id: 'desktop-command',
          kind: 'execute',
          title: 'pwd',
          status: 'completed',
        });
        fixture.append({ type: 'turn_completed', turn_sequence: 1 });
        await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
        await expectPaintedDesktop();
        const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
        await expect(computer).toBeVisible();
        await computer.getByRole('tab', { name: 'Terminal' }).click();
        await expect(desktop).toHaveCount(0);
        await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
        await expectPaintedDesktop();
        await computer.getByRole('button', { name: 'Close computer', exact: true }).click();
        await expect(computer).toHaveCount(0);
        await expect(desktop).toHaveCount(0);
        await expect(message).toHaveValue('Open a terminal and run pwd where I can see it.');
        expect(fixture.writes).toEqual([]);
        expect(fixture.boundary.writes).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });
  }
}

for (const reducedMotion of ['no-preference', 'reduce'] as const) {
  test(`LOCAL desktop loading and timed handoffs with motion ${reducedMotion}`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
    try {
      await page.setViewportSize({ width: 1600, height: 1000 });
      await page.emulateMedia({ reducedMotion });
      await fixture.route(page, baseURL!);
      const desktopStream = await desktopBoundary(page, false);
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
      const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
      const control = desktop.getByRole('button', { name: 'Take control', exact: true });
      await expect(desktop.getByRole('status')).toContainText('Loading the desktop screen');
      await expect(control).toBeDisabled();
      await expect(desktop.locator('.agent-desktop__loading')).toBeVisible();
      const shimmer = await desktop
        .locator('.skeleton')
        .first()
        .evaluate((e) => getComputedStyle(e, '::after').animationName);
      expect(shimmer).toBe(reducedMotion === 'reduce' ? 'none' : 'skeleton-shimmer');
      await page.screenshot({ path: info.outputPath(`loading-${reducedMotion}.png`) });
      desktopStream.paint();
      await expect(control).toBeEnabled();
      await expect(desktop.locator('.agent-desktop__loading')).toHaveCount(0);
      await page.clock.install();
      // Leave ample room for real screenshot/protocol work; component tests
      // cover the exact 4,999/5,000ms boundary with a fully stopped clock.
      await control.click();
      await expect(desktop.locator('.agent-desktop__notice')).toContainText('You have control');
      expect(
        await desktop
          .locator('.agent-desktop__notice')
          .evaluate((e) => getComputedStyle(e).pointerEvents),
      ).toBe('none');
      await page.clock.fastForward(3_000);
      await expect(desktop.locator('.agent-desktop__notice')).toBeVisible();
      await desktop.locator('canvas').click();
      await page.keyboard.down('Control');
      await page.mouse.down();
      await expect
        .poll(() =>
          desktopStream.inputs.some((x) => x.kind === 'key' && x.down && x.keysym === 0xffe3),
        )
        .toBe(true);
      fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      fixture.live('text_delta', 'Using the shared desktop.');
      await expect(desktop.getByRole('status')).toContainText(
        'Agent is in control · Working in Desktop',
      );
      await expect(control).toBeEnabled();
      await expect(desktop.locator('.agent-desktop__notice')).toContainText(
        'The Agent has control',
      );
      await expect
        .poll(() =>
          desktopStream.inputs.some((x) => x.kind === 'key' && !x.down && x.keysym === 0xffe3),
        )
        .toBe(true);
      expect(desktopStream.inputs.filter((x) => x.kind === 'pointer').at(-1)?.buttons).toBe(0);
      await page.mouse.up();
      await page.keyboard.up('Control');
      await page.screenshot({ path: info.outputPath(`handoff-${reducedMotion}.png`) });
      await page.clock.fastForward(3_000);
      await expect(desktop.locator('.agent-desktop__notice')).toBeVisible();
      await page.clock.fastForward(2_400);
      await expect(desktop.locator('.agent-desktop__notice')).toHaveCount(0);
      await expect(desktop.getByRole('status')).toContainText(
        'Agent is in control · Working in Desktop',
      );
      expect(fixture.writes).toEqual([]);
      expect(desktopStream.connections).toHaveLength(1);
    } finally {
      await fixture.close(page);
    }
  });
}

test('LOCAL Workspace tabs and the 1180px sidebar threshold respect keyboard and manual choices', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
  try {
    await page.setViewportSize({ width: 1180, height: 900 });
    await fixture.route(page, baseURL!);
    await desktopBoundary(page);
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
    const sidebar = page.getByRole('complementary', {
      name: 'Application navigation',
      exact: true,
    });
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    const screen = computer.getByRole('tab', { name: 'Screen', exact: true });
    const files = computer.getByRole('tab', { name: 'Files', exact: true });
    await expect(screen).toHaveAttribute('aria-selected', 'true');
    await expect(screen).toHaveAttribute('tabindex', '0');
    await expect(files).toHaveAttribute('tabindex', '-1');
    await expect(computer.getByRole('tab', { name: 'Terminal' })).toHaveCount(0);
    await screen.focus();
    await page.keyboard.press('End');
    await expect(files).toBeFocused();
    await expect(files).toHaveAttribute('aria-selected', 'true');
    await expect(screen).toHaveAttribute('tabindex', '-1');
    await expect(computer.getByRole('button', { name: 'Reconnect' })).toHaveCount(0);
    await page.keyboard.press('Home');
    await expect(screen).toBeFocused();
    await expect(computer.getByRole('button', { name: 'Reconnect' })).toBeVisible();
    await expect(sidebar).not.toHaveClass(/sidebar--collapsed/);
    await page.setViewportSize({ width: 1179, height: 900 });
    await expect(sidebar).toHaveClass(/sidebar--collapsed/);
    await page.getByRole('button', { name: 'Show sidebar', exact: true }).click();
    await expect(sidebar).not.toHaveClass(/sidebar--collapsed/);
    await page.setViewportSize({ width: 1100, height: 900 });
    await expect(sidebar).not.toHaveClass(/sidebar--collapsed/);
    await sidebar.getByRole('button', { name: 'Hide navigation', exact: true }).click();
    await page.setViewportSize({ width: 1440, height: 900 });
    await expect(sidebar).toHaveClass(/sidebar--collapsed/);
    const conversation = page.locator('.agent-surface');
    expect((await conversation.boundingBox())!.width).toBeGreaterThanOrEqual(420);
    expect(
      (await page.locator('.agent-toolbar__heading').boundingBox())!.width,
    ).toBeGreaterThanOrEqual(96);
    await computer.getByRole('button', { name: 'Close computer' }).click();
    await page.getByRole('button', { name: 'Show Agent computer' }).click();
    await expect(screen).toHaveAttribute('aria-selected', 'true');
    expect(fixture.writes).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});
