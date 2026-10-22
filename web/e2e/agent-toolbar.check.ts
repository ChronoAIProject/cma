import { test, expect } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';

for (const width of [1600, 390]) {
  for (const theme of ['light', 'dark']) {
    test(`LOCAL compact toolbar and Files retain live chat at ${width}px in ${theme}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      try {
        await page.setViewportSize({ width, height: 1000 });
        await fixture.route(page, baseURL!);
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
        await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
          route.fulfill({
            json: {
              path: '',
              entries: [
                {
                  path: 'README.md',
                  name: 'README.md',
                  kind: 'file',
                  size_bytes: 8,
                  hidden: false,
                },
                {
                  path: 'long-file-name-that-needs-to-stay-in-one-row.txt',
                  name: 'long-file-name-that-needs-to-stay-in-one-row.txt',
                  kind: 'file',
                  size_bytes: 8,
                  hidden: false,
                },
              ],
            },
          }),
        );
        await page.route('**/api/v1/agents/agt_contract/files/content**', (route) =>
          route.fulfill({
            json: {
              path: 'README.md',
              content_base64: Buffer.from('# Notes\n').toString('base64'),
              size_bytes: 8,
              sha256: 'a'.repeat(64),
            },
          }),
        );
        await page.goto('/agents/agt_contract');
        const message = page.getByRole('textbox', { name: 'Message', exact: true });
        await expect(message).toBeEnabled();
        await page.evaluate(
          (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
          theme,
        );
        await message.fill('Keep this message and conversation while browsing files.');
        const field = await message.elementHandle();
        await page.getByRole('button', { name: 'Show files', exact: true }).click();
        const files = page.getByRole('complementary', { name: 'Agent files', exact: true });
        await expect(files.getByRole('button', { name: 'README.md' })).toBeVisible();
        await expect(page.getByRole('dialog', { name: 'Files' })).toHaveCount(0);
        if (width === 1600) {
          const separator = page.getByRole('separator', { name: 'Resize the details column' });
          await separator.focus();
          await page.keyboard.press('ArrowLeft');
          await expect(separator).toHaveAttribute('aria-valuenow', '316');
          const bounds = (await separator.boundingBox())!;
          await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + 100);
          await page.mouse.down();
          await page.mouse.move(bounds.x - 80, bounds.y + 100, { steps: 8 });
          await page.mouse.up();
          await expect
            .poll(async () => Number(await separator.getAttribute('aria-valuenow')))
            .toBeGreaterThan(380);
        }
        await expect
          .poll(() =>
            files.evaluate((node) => {
              const box = node.getBoundingClientRect();
              return (
                box.left >= 0 &&
                box.right <= innerWidth + 1 &&
                box.bottom <= innerHeight + 1 &&
                node.scrollWidth <= node.clientWidth + 1
              );
            }),
          )
          .toBe(true);
        await files.getByRole('button', { name: 'README.md' }).click();
        await expect(files.getByRole('textbox', { name: 'Contents of README.md' })).toHaveValue(
          '# Notes\n',
        );
        await page.screenshot({ path: info.outputPath(`toolbar-files-${width}-${theme}.png`) });
        await files.getByRole('button', { name: 'Close files', exact: true }).click();
        await expect(message).toHaveValue(
          'Keep this message and conversation while browsing files.',
        );
        expect(await message.evaluate((node, original) => node === original, field)).toBe(true);
        await page.getByRole('button', { name: 'Show files', exact: true }).click();
        await expect(files.getByRole('textbox', { name: 'Contents of README.md' })).toHaveValue(
          '# Notes\n',
        );
        await files.getByRole('button', { name: 'Close files', exact: true }).click();
        expect(fixture.writes).toEqual([]);
        expect(fixture.boundary.writes).toEqual([]);
        expect(fixture.boundary.unexpected).toEqual([]);
        expect(fixture.boundary.errors).toEqual([]);
        expect(fixture.errors).toEqual([]);
      } catch (error) {
        console.log({
          body: await page.locator('body').innerText(),
          errors: fixture.errors,
          unexpected: fixture.boundary.unexpected,
          boundaryErrors: fixture.boundary.errors,
        });
        throw error;
      } finally {
        await fixture.close(page);
      }
    });
  }
}

test('LOCAL Agent rename and system prompt updates keep the conversation mounted', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  let preferences = {
    title: 'Original Agent',
    system_prompt: 'Be concise.',
    prompt_supported: true,
    prompt_pending: false,
    revision: 1,
  };
  const writes: unknown[] = [];
  try {
    await fixture.route(page, baseURL!);
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
    await page.route('**/api/v1/agents/agt_contract/preferences', (route) => {
      if (route.request().method() === 'PATCH') {
        const body = route.request().postDataJSON() as {
          title?: string;
          system_prompt?: string;
          expected_revision: number;
        };
        expect(body.expected_revision).toBe(preferences.revision);
        expect(route.request().headers()['idempotency-key']).toBeTruthy();
        writes.push(body);
        preferences = { ...preferences, ...body, revision: preferences.revision + 1 };
      }
      return route.fulfill({ json: preferences });
    });
    await page.goto('/agents/agt_contract');
    const message = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(message).toBeEnabled();
    await message.fill('Keep my draft while changing Agent preferences.');
    const original = await message.elementHandle();
    await page.getByRole('button', { name: 'More Agent actions', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Rename Agent', exact: true }).click();
    await page.getByRole('textbox', { name: 'Agent name', exact: true }).fill('Research partner');
    await page
      .getByRole('dialog', { name: 'Rename Agent' })
      .getByRole('button', { name: 'Save', exact: true })
      .click();
    await expect(
      page.getByRole('heading', { name: 'Research partner', exact: true }),
    ).toBeVisible();
    await page.getByRole('button', { name: 'More Agent actions', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Update system prompt', exact: true }).click();
    await page
      .getByRole('textbox', { name: 'Agent system prompt', exact: true })
      .fill('Be precise and cite your sources.');
    await page
      .getByRole('dialog', { name: 'Update system prompt' })
      .getByRole('button', { name: 'Save', exact: true })
      .click();
    await expect(page.getByRole('dialog', { name: 'Update system prompt' })).toHaveCount(0);
    expect(writes).toHaveLength(2);
    await expect(message).toHaveValue('Keep my draft while changing Agent preferences.');
    expect(await message.evaluate((node, previous) => node === previous, original)).toBe(true);
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});
