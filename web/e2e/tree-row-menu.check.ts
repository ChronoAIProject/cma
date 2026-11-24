import { expect, test as base, type Locator, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

const test = base.extend({
  page: async ({ page, browserName, hasTouch }, runTest) => {
    // Chromium can lose initial touch emulation while creating the mobile page.
    const emulation =
      browserName === 'chromium' && hasTouch ? await page.context().newCDPSession(page) : undefined;
    try {
      await emulation?.send('Emulation.setTouchEmulationEnabled', { enabled: true });
      await runTest(page);
    } finally {
      // Detaching resets native touch emulation, so keep it through all interactions.
      await emulation?.detach();
    }
  },
});

async function pointerMode(page: Page, mobile: boolean) {
  expect(
    await page.evaluate(() => ({
      coarse: matchMedia('(pointer: coarse)').matches,
      hover: matchMedia('(hover: hover)').matches,
      touch: navigator.maxTouchPoints > 0,
    })),
  ).toEqual({ coarse: mobile, hover: !mobile, touch: mobile });
}

async function fixture(page: Page, baseURL: string, mobile: boolean, actions = true) {
  await pointerMode(page, mobile);
  const boundary = new LocalStartup();
  boundary.agent = { ...boundary.agent, status: 'ready', title: 'Menu fixture task' };
  boundary.workspace = {
    ...boundary.workspace,
    agents: boundary.workspace.agents.map((agent) => ({
      ...agent,
      status: 'ready',
      title: 'Menu fixture task',
    })),
  };
  await boundary.route(page, baseURL);
  if (!actions)
    await page.route('**/auth/session', (route) =>
      route.fulfill({
        json: {
          authenticated: true,
          subject: 'local-owner',
          display_name: 'Read-only fixture',
          capabilities: [],
        },
      }),
    );
  await page.goto('/agent-manager');
  if (mobile) {
    const navigation = page.getByRole('button', { name: 'Show navigation', exact: true });
    await expect(navigation).toBeVisible();
    await navigation.click();
  }
  const row = page
    .getByRole('tree', { name: 'Personal Agents', exact: true })
    .locator(`[data-node-id="agent:${boundary.agent.id}"]`);
  await expect(row).toBeVisible();
  await pointerMode(page, mobile);
  return { boundary, row };
}

async function untouched(row: Locator) {
  await expect(row).toHaveAttribute('aria-selected', 'false');
  expect(await row.evaluate((node) => node.matches(':hover, :focus-within'))).toBe(false);
}

async function visibleButton(button: Locator) {
  await expect(button).toBeVisible();
  await expect(button).toHaveCSS('visibility', 'visible');
  await expect(button).toHaveCSS('opacity', '1');
}

function noEffects(boundary: LocalStartup) {
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
}

for (const touch of [true, false]) {
  test.describe(touch ? '390px coarse pointer' : '1440px fine pointer', () => {
    test.use({
      viewport: { width: touch ? 390 : 1440, height: 900 },
      hasTouch: touch,
      isMobile: touch,
    });

    test('LOCAL row Options are discoverable and retain pointer and keyboard operation', async ({
      page,
      baseURL,
    }) => {
      const { boundary, row } = await fixture(page, baseURL!, touch);
      expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(touch);
      expect(await page.evaluate(() => matchMedia('(hover: hover)').matches)).toBe(!touch);
      await untouched(row);
      const button = row.getByRole('button', {
        name: 'Options for Menu fixture task',
        exact: true,
        includeHidden: true,
      });
      if (touch) {
        await visibleButton(button);
        const box = await button.boundingBox();
        expect(box!.width).toBeGreaterThanOrEqual(44);
        expect(box!.height).toBeGreaterThanOrEqual(44);
        expect(
          await button.evaluate((node) => {
            const box = node.getBoundingClientRect();
            return node.contains(
              document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2),
            );
          }),
        ).toBe(true);
        await button.tap();
      } else {
        await expect(button).toHaveCSS('visibility', 'visible');
        await expect(button).toHaveCSS('opacity', '0');
        await row.hover();
        await visibleButton(button);
        await button.click();
        await page.mouse.move(1400, 850);
        await untouched(row);
      }
      const menu = page.getByRole('menu', { name: 'Actions for Menu fixture task', exact: true });
      await expect(menu).toBeVisible();
      await expect(button).toHaveAttribute('aria-expanded', 'true');
      await visibleButton(button);
      await expect(menu.getByRole('menuitem', { name: 'Stop', exact: true })).toBeFocused();
      await expect(menu.getByRole('menuitem', { name: /^Delete/ })).toHaveAttribute(
        'aria-disabled',
        'false',
      );
      await page.keyboard.press('Escape');
      await expect(menu).toHaveCount(0);
      await expect(button).toBeFocused();
      await row.focus();
      await page.keyboard.press('Tab');
      await expect(button).toBeFocused();
      await visibleButton(button);
      await page.keyboard.press('Enter');
      await expect(menu.getByRole('menuitem', { name: 'Stop', exact: true })).toBeFocused();
      await page.keyboard.press('Enter');
      const confirmation = page.getByRole('dialog', { name: 'Stop Agent', exact: true });
      await expect(confirmation).toBeVisible();
      noEffects(boundary);
      await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(confirmation).toHaveCount(0);
      if (touch) await button.tap();
      else await button.click();
      await menu.getByRole('menuitem', { name: /^Delete/ }).click();
      const deletion = page.getByRole('dialog', { name: 'Delete Agent', exact: true });
      await expect(deletion).toBeVisible();
      await expect(deletion.getByRole('checkbox')).not.toBeChecked();
      await expect(
        deletion.getByRole('button', { name: 'Stop and delete', exact: true }),
      ).toBeDisabled();
      noEffects(boundary);
      await deletion.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(deletion).toHaveCount(0);
      await pointerMode(page, touch);
      noEffects(boundary);
    });

    test('LOCAL a row without permitted actions has no Options control', async ({
      page,
      baseURL,
    }) => {
      const { boundary, row } = await fixture(page, baseURL!, touch, false);
      await expect(
        row.getByRole('button', { name: /^Options for/, includeHidden: true }),
      ).toHaveCount(0);
      await row.focus();
      await expect(
        row.getByRole('button', { name: /^Options for/, includeHidden: true }),
      ).toHaveCount(0);
      await pointerMode(page, touch);
      noEffects(boundary);
    });
  });
}
