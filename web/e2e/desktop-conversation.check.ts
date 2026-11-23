import {
  registerComputerSkillsRedesignCases,
  registerDesktopMotionCases,
} from './redesign-agent.cases';
import { test, expect } from '@playwright/test';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';

import { desktopBoundary } from './desktop-rfb.fixture';

// Exercise the same assertions under slower CI scheduling without changing timeouts.
const cpuRate = Number(process.env.CMA_E2E_CPU_RATE ?? '1');
if (!Number.isFinite(cpuRate) || cpuRate < 1 || cpuRate > 6)
  throw new Error('CMA_E2E_CPU_RATE must be between 1 and 6.');
test.beforeEach(async ({ context, page }) => {
  if (cpuRate === 1) return;
  const session = await context.newCDPSession(page);
  await session.send('Emulation.setCPUThrottlingRate', { rate: cpuRate });
});

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
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
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
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
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
        await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
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
        await computer.getByRole('tab', { name: 'Operations' }).click();
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

registerDesktopMotionCases();

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
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
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
    await expect(computer.getByRole('tab', { name: 'Operations' })).toHaveCount(0);
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

test('LOCAL running desktop takeover confirms and waits for the public cancelled-turn event before input', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
  try {
    await page.setViewportSize({ width: 1440, height: 900 });
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
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
    await expect(desktop.getByRole('button', { name: 'Take control' })).toBeEnabled();
    await expect.poll(() => fixture.streams.size).toBeGreaterThan(0);
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    await expect(desktop.getByRole('status')).toContainText('Agent is in control');
    await desktop.getByRole('button', { name: 'Take control' }).click();
    await expect(
      page.getByText("Taking control stops the Agent's current turn", { exact: true }),
    ).toBeVisible();
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    expect(fixture.writes).toEqual([]);
    await desktop.getByRole('button', { name: 'Take control' }).click();
    await page.getByRole('button', { name: 'Stop Agent and take control', exact: true }).click();
    await expect
      .poll(() => fixture.writes.filter((write) => write.path.endsWith('/controls')).length)
      .toBe(1);
    expect(fixture.writes[0]!.body.command).toEqual({ type: 'cancel' });
    await expect(page.getByText('Waiting for the Agent to stop…', { exact: true })).toBeVisible();
    await desktop.locator('canvas').click();
    await page.keyboard.press('a');
    expect(stream.inputs.some((input) => input.kind === 'key' && input.down)).toBe(false);
    fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
    await expect(desktop.getByRole('button', { name: 'Return to Agent' })).toBeVisible();
    await expect(desktop.locator('canvas')).toBeFocused();
    await page.keyboard.press('b');
    await expect
      .poll(() => stream.inputs.some((input) => input.kind === 'key' && input.down))
      .toBe(true);
    expect(fixture.writes.filter((write) => write.path.endsWith('/controls'))).toHaveLength(1);
  } finally {
    await fixture.close(page);
  }
});

for (const completion of ['before End', 'after End'] as const) {
  test(`LOCAL late conversation history preserves computer keyboard focus ${completion}`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
    let resume!: () => void;
    const history = new Promise<void>((resolve) => {
      resume = resolve;
    });
    let waiting = false;
    try {
      await page.setViewportSize({ width: 1180, height: 900 });
      await fixture.route(page, baseURL!);
      await desktopBoundary(page);
      await page.route('**/api/v2/ui/task-experience', (route) =>
        route.fulfill({
          json: {
            object: 'cma.ui.task_experience',
            schema_version: 1,
            account_id: 'local-owner',
            capabilities: {
              task_home_admission: false,
              suggested_followup_admission: false,
              desktop_enable: false,
              browser_takeover: false,
              replay_presentation: false,
            },
          },
        }),
      );
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
      await page.route('**/api/v2/agents/agt_contract/events/snapshot**', async (route) => {
        waiting = true;
        await history;
        await route.fallback();
      });
      await page.goto('/agents/agt_contract');
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
      const screen = computer.getByRole('tab', { name: 'Screen', exact: true });
      const files = computer.getByRole('tab', { name: 'Files', exact: true });
      await expect.poll(() => waiting).toBe(true);
      await expect(screen).toHaveAttribute('aria-selected', 'true');
      await screen.focus();
      if (completion === 'after End') {
        await page.keyboard.press('End');
        await expect(files).toBeFocused();
        await expect(files).toHaveAttribute('aria-selected', 'true');
      }
      await expect(page.getByText('Loading conversation activity.', { exact: true })).toBeVisible();
      resume();
      await expect(page.getByText('Loading conversation activity.', { exact: true })).toHaveCount(
        0,
      );
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
      await expect(completion === 'before End' ? screen : files).toBeFocused();
      if (completion === 'before End') await page.keyboard.press('End');
      await expect(files).toBeFocused();
      await expect(files).toHaveAttribute('aria-selected', 'true');
      await expect(files).toHaveAttribute('tabindex', '0');
      for (const [key, selected, other] of [
        ['Home', screen, files],
        ['ArrowRight', files, screen],
        ['ArrowLeft', screen, files],
      ] as const) {
        await page.keyboard.press(key);
        await expect(selected).toBeFocused();
        await expect(selected).toHaveAttribute('aria-selected', 'true');
        await expect(selected).toHaveAttribute('tabindex', '0');
        await expect(other).toHaveAttribute('tabindex', '-1');
      }
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      resume();
      await fixture.close(page);
    }
  });
}

registerComputerSkillsRedesignCases();
