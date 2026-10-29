import { test, expect } from '@playwright/test';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';

import { desktopBoundary } from './desktop-rfb.fixture';

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
      const control = desktop.getByRole('button', { name: 'Control desktop', exact: true });
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
      await expect(desktop.getByRole('status')).toContainText('Agent is working');
      await expect(control).toBeDisabled();
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
      await expect(desktop.getByRole('status')).toContainText('Agent is working');
      expect(fixture.writes).toEqual([]);
      expect(desktopStream.connections).toHaveLength(1);
    } finally {
      await fixture.close(page);
    }
  });
}
