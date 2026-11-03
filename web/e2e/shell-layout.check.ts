import { test, expect, type Locator, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalAgentV2, binding } from './agent-v2.fixture';

/** The collapsed sidebar's width: none on a phone (§9), the 56px rail elsewhere. */
const rail = (viewport: number) => (viewport < 640 ? 0 : 56);

async function settleShell(page: Page, viewport: number, sidebar: number) {
  // Both tracks must settle: the collapsed sidebar keeps its width when
  // resizing between phones, while the center still has a 300ms transition
  // to its new available width.
  await expect
    .poll(() =>
      page.evaluate(() => {
        const rail = document.querySelector('.frame__sidebar')!.getBoundingClientRect();
        const center = document.querySelector('.frame__center')!.getBoundingClientRect();
        return { rail: rail.width, right: center.right };
      }),
    )
    .toEqual({ rail: sidebar, right: viewport });
}

async function insideViewport(element: Locator) {
  await expect
    .poll(() =>
      element.evaluate((node) => {
        const box = node.getBoundingClientRect();
        return (
          box.width > 0 &&
          box.left >= 0 &&
          box.right <= innerWidth &&
          box.top >= 0 &&
          box.bottom <= innerHeight
        );
      }),
    )
    .toBe(true);
}

for (const width of [390, 360, 320]) {
  for (const running of [false, true]) {
    test(`LOCAL cold ${width}px shell keeps ${running ? 'active' : 'idle'} conversation controls usable`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      if (running) {
        fixture.queued.enabled = true;
        fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      }
      try {
        await page.setViewportSize({ width, height: 844 });
        await fixture.route(page, baseURL!);
        await page.route('**/api/v2/agents/agt_contract/queued-inputs', (route) =>
          route.fulfill({
            json: {
              object: 'cma.agent.queued_inputs',
              agent_id: 'agt_contract',
              data: [],
              next_after: null,
            },
          }),
        );
        await page.route('**/api/v1/agents/agt_contract/models', (route) =>
          route.fulfill({ json: { models: [{ id: 'gpt-6', label: 'GPT-6', available: true }] } }),
        );
        await page.goto('/agents/agt_contract');
        const field = page.getByRole('textbox', { name: 'Message', exact: true });
        const send = page.getByRole('button', {
          name: running ? 'Queue message' : 'Send message',
          exact: true,
        });
        const attach = page.getByRole('button', { name: 'Attach files', exact: true });
        await expect(field).toBeEnabled();
        await settleShell(page, width, rail(width));
        await field.fill('Keep this draft while resizing the full shell.');
        // Steering is offered only while a turn runs.
        const steering = page.getByRole('button', { name: /^Steering mode/ });
        await expect(steering).toHaveCount(running ? 1 : 0);
        for (const element of running ? [field, send, attach, steering] : [field, send, attach]) {
          await insideViewport(element);
        }
        expect((await field.boundingBox())!.width).toBeGreaterThanOrEqual(140);
        for (const button of [send, attach]) {
          expect((await button.boundingBox())!.width).toBeGreaterThanOrEqual(44);
          expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
        }
        if (running) {
          const stop = page.getByRole('button', { name: 'Stop response', exact: true });
          await expect(stop).toBeEnabled();
          await insideViewport(stop);
          expect((await stop.boundingBox())!.width).toBeGreaterThanOrEqual(44);
          expect((await send.boundingBox())!.y).toBe((await stop.boundingBox())!.y);
        }
        const form = page.getByRole('form', { name: 'Message Agent' });
        const box = (await page.locator('.agent-composer__box').boundingBox())!;
        for (const control of [
          form.getByRole('button', { name: /^Model and reasoning effort/ }),
          form.getByRole('button', { name: 'Permission mode', exact: true }),
          form.getByRole('button', { name: 'Context usage', exact: true }),
        ]) {
          await insideViewport(control);
          // Every control sits inside the composer box, with no row below it.
          const bounds = (await control.boundingBox())!;
          expect(bounds.y).toBeGreaterThanOrEqual(box.y);
          expect(bounds.y + bounds.height).toBeLessThanOrEqual(box.y + box.height);
          expect(bounds.height).toBeGreaterThanOrEqual(44);
        }
        await page.getByRole('button', { name: 'More Agent actions' }).click();
        await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
        await page.getByRole('button', { name: 'Close Agent details', exact: true }).click();
        await expect(field).toHaveValue('Keep this draft while resizing the full shell.');
        await settleShell(page, width, rail(width));
        await insideViewport(field);
        await page.screenshot({
          path: info.outputPath(`cold-${width}-${running ? 'active' : 'idle'}.png`),
        });
        expect(fixture.writes).toEqual([]);
        expect(fixture.boundary.writes).toEqual([]);
        expect(fixture.boundary.unexpected).toEqual([]);
        expect(fixture.boundary.errors).toEqual([]);
        expect(fixture.errors).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });
  }
}

test('LOCAL shell resize preserves draft, desktop navigation width and manual collapse', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.setViewportSize({ width: 1440, height: 844 });
  await page.goto('/agents/agt_contract');
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(input).toBeEnabled();
  await input.fill('Keep this draft while resizing the full shell.');
  await settleShell(page, 1440, 344);
  const handle = (await page.getByRole('separator', { name: 'Resize navigation' }).boundingBox())!;
  const startX = handle.x + handle.width / 2;
  await page.mouse.move(startX, handle.y + 40);
  await page.mouse.down();
  await page.mouse.move(startX + 56, handle.y + 40, { steps: 4 });
  await page.mouse.up();
  await settleShell(page, 1440, 400);
  for (const width of [390, 320, 1440]) {
    await page.setViewportSize({ width, height: 844 });
    await settleShell(page, width, width < 1024 ? rail(width) : 400);
    await insideViewport(input);
    await expect(input).toHaveValue('Keep this draft while resizing the full shell.');
  }
  await page.getByRole('button', { name: 'Hide navigation', exact: true }).click();
  await settleShell(page, 1440, 56);
  for (const width of [320, 1440]) {
    await page.setViewportSize({ width, height: 844 });
    await settleShell(page, width, rail(width));
    await expect(input).toHaveValue('Keep this draft while resizing the full shell.');
  }
  await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
  await settleShell(page, 1440, 400);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

for (const authenticated of [false, true]) {
  test(`LOCAL landing preserves ${authenticated ? 'signed in task navigation' : 'signed out compact navigation'}`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    if (!authenticated)
      await page.route('**/auth/session', (route) =>
        route.fulfill({ json: { authenticated: false } }),
      );
    await page.goto('/');
    if (authenticated) {
      await expect(page.getByRole('heading', { name: 'What would you like to do?' })).toBeVisible();
      await page.getByLabel('Your task').fill('Retain this Home draft across screen sizes.');
      await page.getByRole('button', { name: 'Hide navigation', exact: true }).click();
    } else await expect(page.getByRole('button', { name: 'Get Started' })).toBeVisible();
    for (const width of [1440, 800, 320]) {
      await page.setViewportSize({ width, height: 900 });
      await settleShell(page, width, rail(width));
      if (authenticated) {
        await expect(page.getByLabel('Your task')).toHaveValue(
          'Retain this Home draft across screen sizes.',
        );
        await expect(
          page.getByRole('button', { name: 'Show navigation', exact: true }),
        ).toBeVisible();
      } else
        await expect(page.getByRole('button', { name: /(?:Show|Hide) navigation/ })).toHaveCount(0);
      await expect(page.getByRole('separator', { name: 'Resize navigation' })).toHaveCount(
        authenticated ? 1 : 0,
      );
      await expect(page.locator('.frame[data-sidebar-overlay]')).toHaveCount(0);
    }
    if (authenticated) {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
      await expect(
        page.getByRole('button', { name: 'Hide navigation', exact: true }),
      ).toBeVisible();
      await page.getByRole('link', { name: 'CMA home' }).click();
      await expect(page.getByRole('heading', { name: 'What would you like to do?' })).toBeVisible();
      await expect(page.getByLabel('Your task')).toHaveValue(
        'Retain this Home draft across screen sizes.',
      );
    }
    expect(boundary.writes).toEqual([]);
  });
}

test('LOCAL system text scales user and Agent content together without changing message geometry', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalAgentV2();
  fixture.append({ type: 'message', role: 'user', content: 'Review this example.' });
  fixture.append({
    type: 'message',
    role: 'assistant',
    content:
      '# Example\n\nReadable explanation with `inline code`.\n\n```text\nconst value = 42;\n```\n\n- A useful list item\n\n| Name | Value |\n| --- | --- |\n| example | 42 |',
  });
  try {
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/queued-inputs', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.queued_inputs',
          agent_id: 'agt_contract',
          data: [],
          next_after: null,
        },
      }),
    );
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto('/agents/agt_contract');
    await expect(page.locator('.agent-content h2')).toHaveText('Example');
    await expect(
      page.locator('.message--user').getByRole('button', { name: /vote message/ }),
    ).toHaveCount(0);
    await expect(
      page.locator('.message--assistant').getByRole('button', { name: /vote message/ }),
    ).toHaveCount(2);
    const measurements = () =>
      page.evaluate(() => {
        const sample = (selector: string) => {
          const element = document.querySelector(selector)!;
          const style = getComputedStyle(element);
          return { size: parseFloat(style.fontSize), line: parseFloat(style.lineHeight) };
        };
        return {
          text: [
            '.message--user p',
            '.agent-content p',
            '.agent-content h2',
            '.agent-content code',
            '.agent-content pre',
            '.agent-content li',
            '.agent-content table',
          ].map(sample),
          avatar: document.querySelector('.message__avatar')!.getBoundingClientRect().width,
          padding: getComputedStyle(document.querySelector('.message-stream')!).paddingLeft,
          pageOverflow: document.documentElement.scrollWidth > innerWidth,
        };
      });
    const baseline = await measurements();
    for (const [name, factor] of [
      ['Small', 0.9],
      ['Large', 1.15],
      ['Medium', 1],
    ] as const) {
      await page.getByRole('button', { name: /account menu/i }).click();
      await page
        .getByRole('group', { name: 'System text size' })
        .getByRole('menuitemradio', { name })
        .click();
      await page.keyboard.press('Escape');
      const current = await measurements();
      for (let index = 0; index < baseline.text.length; index++) {
        expect(current.text[index]!.size).toBeCloseTo(baseline.text[index]!.size * factor, 2);
        expect(current.text[index]!.line).toBeCloseTo(baseline.text[index]!.line * factor, 2);
      }
      expect(current.avatar).toBe(baseline.avatar);
      expect(current.padding).toBe(baseline.padding);
      expect(current.pageOverflow).toBe(false);
      await page.setViewportSize({ width: 320, height: 900 });
      await settleShell(page, 320, 0);
      await insideViewport(page.getByRole('textbox', { name: 'Message', exact: true }));
      expect((await measurements()).pageOverflow).toBe(false);
      await page.screenshot({
        path: info.outputPath(`conversation-${name.toLowerCase()}-320.png`),
      });
      await page.setViewportSize({ width: 1440, height: 900 });
      await settleShell(page, 1440, 344);
    }
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.writes).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});
