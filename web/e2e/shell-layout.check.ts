import { test, expect, type Locator, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalAgentV2, binding } from './agent-v2.fixture';

async function settleShell(page: Page, viewport: number, sidebar: number) {
  // Both tracks must settle: the rail stays 56px when resizing between phones,
  // while the center still has a 300ms transition to its new available width.
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
        await settleShell(page, width, 56);
        await field.fill('Keep this draft while resizing the full shell.');
        for (const element of [field, send, attach, page.locator('.agent-composer__mode')]) {
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
        for (const name of ['Model', 'Reasoning effort', 'Permission mode']) {
          await insideViewport(
            page
              .getByRole('form', { name: 'Message Agent' })
              .getByRole('button', { name, exact: true }),
          );
        }
        const inputSeat = (await page.locator('.agent-composer__capsule').boundingBox())!;
        const usage = (await page.getByRole('region', { name: 'Agent usage' }).boundingBox())!;
        expect(usage.x).toBe(inputSeat.x);
        expect(usage.width).toBe(inputSeat.width);
        await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
        await page.getByRole('button', { name: 'Close Agent details', exact: true }).click();
        await expect(field).toHaveValue('Keep this draft while resizing the full shell.');
        await settleShell(page, width, 56);
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
    await settleShell(page, width, width < 1024 ? 56 : 400);
    await insideViewport(input);
    await expect(input).toHaveValue('Keep this draft while resizing the full shell.');
  }
  await page.getByRole('button', { name: 'Hide navigation', exact: true }).click();
  await settleShell(page, 1440, 56);
  for (const width of [320, 1440]) {
    await page.setViewportSize({ width, height: 844 });
    await settleShell(page, width, 56);
    await expect(input).toHaveValue('Keep this draft while resizing the full shell.');
  }
  await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
  await settleShell(page, 1440, 400);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
