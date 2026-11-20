import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

const cases = [
  { id: 'research', name: 'Research a topic', fields: [['Topic', 'Synthetic research', 'topic']] },
  {
    id: 'spreadsheet',
    name: 'Analyze a spreadsheet',
    fields: [['Spreadsheet link', 'https://example.com/data.csv', 'link']],
  },
  {
    id: 'document',
    name: 'Write a document',
    fields: [
      ['Document type', 'report', 'type'],
      ['Subject', 'synthetic data', 'subject'],
      ['Audience', 'readers', 'audience'],
    ],
  },
  { id: 'website', name: 'Build a website', fields: [['Project', 'synthetic project', 'project']] },
  {
    id: 'code',
    name: 'Write code',
    fields: [
      ['Language', 'Python', 'language'],
      ['What should the code do?', 'prints [1, 2, 3]', 'behavior'],
    ],
  },
] as const;
async function settle(page: Page) {
  await expect
    .poll(() =>
      page.locator('.frame__center').evaluate((node) => node.getBoundingClientRect().right),
    )
    .toBe(page.viewportSize()!.width);
  await page.evaluate(() => document.fonts.ready);
}
async function fixture(page: Page, baseURL: string, supported = true) {
  const boundary = new LocalStartup();
  const writes: { body: Record<string, unknown>; key: string | undefined }[] = [];
  await boundary.route(page, baseURL);
  await page.route('**/openapi.json', async (route) => {
    await route.fulfill({
      json: {
        openapi: '3.1.0',
        info: { title: 'Synthetic', version: '1' },
        paths: {},
        'x-cma-task-templates-v1': supported,
        components: {
          schemas: {
            AgentLifecycleCreateRequest: { properties: { initial_message: { type: 'string' } } },
          },
        },
      },
    });
  });
  await page.route('**/api/v2/ui/task-experience', (route) =>
    route.fulfill({
      json: {
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: 'local-owner',
        capabilities: {
          task_home_admission: true,
          desktop_enable: false,
          suggested_followup_admission: false,
          browser_takeover: false,
          replay_presentation: false,
        },
      },
    }),
  );
  await page.route('**/api/agent-manager/v1/agents', (route) => {
    writes.push({
      body: route.request().postDataJSON() as Record<string, unknown>,
      key: route.request().headers()['idempotency-key'],
    });
    return route.abort('failed');
  });
  return { boundary, writes };
}
for (const theme of ['light', 'dark']) {
  for (const item of cases) {
    test(`LOCAL ${theme}: ${item.id} fields validate before one explicit task`, async ({
      page,
      baseURL,
    }, info) => {
      const { boundary, writes } = await fixture(page, baseURL!);
      await page.setViewportSize({ width: theme === 'dark' ? 390 : 1440, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript((theme) => localStorage.setItem('cma:appearance', theme), theme);
      await page.goto('/');
      await settle(page);
      await page.getByRole('button', { name: item.name, exact: true }).click();
      const first = page.getByLabel(item.fields[0][0], { exact: true });
      await expect(first).toBeFocused();
      const start = page.getByRole('button', { name: 'Start task', exact: true });
      await expect(start).toBeDisabled();
      const preview = page.getByRole('region', { name: 'Task preview · read-only' });
      await expect(page.getByRole('heading', { name: 'Task preview · read-only' })).toBeVisible();
      await expect(preview).toHaveText(
        'Fill in the required fields above to generate the task preview.',
      );
      await expect(page.getByPlaceholder('What would you like your Agent to do?')).toHaveCount(0);
      await expect(page.getByText(/↵ to run/)).toHaveCount(0);
      if (item.id === 'document')
        await page.screenshot({
          path: info.outputPath(`empty-${theme}.png`),
          animations: 'disabled',
        });
      await first.press('Control+Enter');
      await expect(first).toHaveAttribute('aria-invalid', 'true');
      await first.press('Meta+Enter');
      await first.evaluate((element: HTMLInputElement) => element.form!.requestSubmit());
      expect(writes).toEqual([]);
      for (const [index, [label, value]] of item.fields.entries()) {
        const field = page.getByLabel(label, { exact: true });
        await field.fill(value);
        await expect(field).toBeFocused();
        if (index < item.fields.length - 1) {
          await expect(preview).toHaveText(
            'Fill in the required fields above to generate the task preview.',
          );
          await expect(start).toBeDisabled();
          await expect(page.getByText(/↵ to run/)).toHaveCount(0);
          if (item.id === 'document' && index === 1)
            await page.screenshot({
              path: info.outputPath(`partial-${theme}.png`),
              animations: 'disabled',
            });
        }
      }
      await page.reload();
      await settle(page);
      for (const [label, value] of item.fields)
        await expect(page.getByLabel(label, { exact: true })).toHaveValue(value);
      await expect(start).toBeEnabled();
      const previewText = await preview.textContent();
      await expect(page.getByText(/↵ to run/)).toBeVisible();
      await expect(preview).not.toHaveAttribute('aria-live');
      expect(
        await preview.evaluate(
          (node) => node.closest('[aria-live], [role="status"], [role="alert"]') === null,
        ),
      ).toBe(true);
      await preview.focus();
      await expect(preview).toBeFocused();
      expect(
        await preview.evaluate((node) => {
          const range = document.createRange();
          range.selectNodeContents(node);
          const selection = getSelection()!;
          selection.removeAllRanges();
          selection.addRange(range);
          return selection.toString();
        }),
      ).toBe(previewText);
      await preview.evaluate(() => getSelection()?.removeAllRanges());
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true);
      const bounds = await page.locator('.task-template-fields').boundingBox();
      expect(bounds!.width).toBeGreaterThanOrEqual(Math.min(page.viewportSize()!.width - 64, 600));
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
      await page.screenshot({
        path: info.outputPath(`${item.id}-${theme}.png`),
        animations: 'disabled',
      });
      await (theme === 'dark' ? preview : first).press(
        theme === 'dark' ? 'Meta+Enter' : 'Control+Enter',
      );
      await expect(page.getByRole('button', { name: 'Recover original task' })).toBeVisible();
      expect(writes).toHaveLength(1);
      expect(writes[0]!.body).toEqual({
        name: 'New task',
        initial_message: previewText,
        template: {
          id: item.id,
          version: 1,
          fields: Object.fromEntries(item.fields.map(([, value, key]) => [key, value])),
        },
      });
      await first.fill('Later draft');
      await page.reload();
      await settle(page);
      await page.getByRole('button', { name: 'Recover original task' }).click();
      await expect.poll(() => writes.length).toBe(2);
      expect(writes[1]).toEqual(writes[0]);
      await expect(first).toHaveValue('Later draft');
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
test('LOCAL unsupported template support preserves input and ordinary bracket text', async ({
  page,
  baseURL,
}, info) => {
  const { writes } = await fixture(page, baseURL!, false);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/');
  await settle(page);
  await page.getByRole('button', { name: 'Research a topic', exact: true }).click();
  await page.getByLabel('Topic', { exact: true }).fill('Synthetic');
  await expect(page.getByRole('heading', { name: 'Task preview · read-only' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Task preview · read-only' })).toHaveText(
    'Research Synthetic and summarize the key findings, with links to sources.',
  );
  await expect(page.getByText(/Template tasks are not available on this service/)).toBeVisible();
  await expect(page.getByText(/↵ to run/)).toHaveCount(0);
  await page.screenshot({
    path: info.outputPath('unsupported-template.png'),
    animations: 'disabled',
  });
  await expect(page.getByRole('button', { name: 'Start task' })).toBeDisabled();
  await page.getByLabel('Topic', { exact: true }).press('Control+Enter');
  expect(writes).toEqual([]);
  await page.getByRole('button', { name: 'Write a task without a template' }).click();
  await page
    .getByLabel('Your task')
    .fill('Discuss [topic], [1,2] and [label](https://example.com)');
  await page.getByRole('button', { name: 'Start task' }).click();
  await expect.poll(() => writes.length).toBe(1);
  expect(writes[0]!.body).not.toHaveProperty('template');
});
