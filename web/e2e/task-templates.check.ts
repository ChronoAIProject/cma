import { selectHomeTemplate } from './home-simplification.fixture';
import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { registerRedesignTemplateCases } from './redesign-home.cases';

registerRedesignTemplateCases();

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
        'x-cma-task-templates-v2': supported,
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
      await selectHomeTemplate(page, item.name);
      const first = page.getByLabel(item.fields[0][0], { exact: true });
      await expect(first).toBeFocused();
      const start = page.getByRole('button', { name: 'Start task', exact: true });
      await expect(start).toBeDisabled();
      const preview = page.getByRole('region', { name: 'Task preview · read-only' });
      await expect(page.getByRole('heading', { name: 'Task preview · read-only' })).toBeVisible();
      await expect(preview).toHaveText(
        'Fill in the required fields above to generate the task preview.',
      );
      await expect(
        page.getByPlaceholder('Describe the job. It gets its own cloud computer to do it.'),
      ).toHaveCount(0);
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
          version: 2,
          fields: Object.fromEntries(item.fields.map(([, value, key]) => [key, value])),
        },
      });
      const followup = page.getByRole('textbox', { name: 'Follow-up message', exact: true });
      await followup.fill('Later draft');
      await page.reload();
      await settle(page);
      await page.getByRole('button', { name: 'Recover original task' }).click();
      await expect.poll(() => writes.length).toBe(2);
      expect(writes[1]).toEqual(writes[0]);
      await expect(followup).toHaveValue('Later draft');
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
  await selectHomeTemplate(page, 'Research a topic');
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

for (const width of [1440, 390]) {
  test(`LOCAL ${width}px partial template drafts survive switching, reload and resume`, async ({
    page,
    baseURL,
  }, info) => {
    const { writes, boundary } = await fixture(page, baseURL!, false);
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto('/');
    await settle(page);
    await page.getByLabel('Your task').fill('My separate free-text draft');
    await selectHomeTemplate(page, 'Write a document');
    await page.getByLabel('Document type', { exact: true }).fill(' report ');
    await page.getByLabel('Subject', { exact: true }).fill('草稿 [subject]');
    const switchMode = page.getByRole('button', { name: 'Write a task without a template' });
    await switchMode.focus();
    await switchMode.press('Enter');
    const dialog = page.getByRole('dialog', { name: 'Keep this template draft?' });
    await expect(dialog).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(dialog).toBeHidden();
    await expect(switchMode).toBeFocused();
    await expect(page.getByLabel('Subject', { exact: true })).toHaveValue('草稿 [subject]');
    await switchMode.press('Enter');
    await expect(dialog).toBeVisible();
    await page.screenshot({
      path: info.outputPath(`template-switch-${width}.png`),
      animations: 'disabled',
    });
    await dialog.getByRole('button', { name: 'Switch and keep draft' }).click();
    await expect(page.getByLabel('Your task')).toHaveValue('My separate free-text draft');
    await expect(page.getByLabel('Your task')).toBeFocused();
    await page.reload();
    await settle(page);
    await page.getByRole('button', { name: 'Resume document template' }).click();
    await expect(page.getByLabel('Document type', { exact: true })).toBeFocused();
    await expect(page.getByLabel('Document type', { exact: true })).toHaveValue(' report ');
    await expect(page.getByLabel('Subject', { exact: true })).toHaveValue('草稿 [subject]');
    await expect(page.getByLabel('Audience', { exact: true })).toHaveValue('');
    await expect(page.getByRole('button', { name: 'Start task' })).toBeDisabled();
    await page.getByLabel('Subject', { exact: true }).press('Control+Enter');
    await expect(page.getByLabel('Audience', { exact: true })).toBeFocused();
    await selectHomeTemplate(page, 'Write code');
    await page.getByLabel('Language', { exact: true }).fill('Python');
    await selectHomeTemplate(page, 'Write a document');
    await expect(page.getByLabel('Subject', { exact: true })).toHaveValue('草稿 [subject]');
    await selectHomeTemplate(page, 'Write code');
    await expect(page.getByLabel('Language', { exact: true })).toHaveValue('Python');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
    expect(writes).toEqual([]);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
  });
}

test('LOCAL complete conversion confirms replacement and keeps the template fields', async ({
  page,
  baseURL,
}) => {
  const { writes } = await fixture(page, baseURL!);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/');
  await settle(page);
  await page.getByLabel('Your task').fill('Keep the existing draft until confirmation');
  await selectHomeTemplate(page, 'Research a topic');
  await page.getByLabel('Topic', { exact: true }).fill('  Synthetic topic  ');
  const preview =
    'Research the following topic:\n\nSynthetic topic\n\nSummarize the key findings, with links to sources.';
  await page.getByRole('button', { name: 'Write a task without a template' }).click();
  const dialog = page.getByRole('dialog', { name: 'Replace your free-text draft?' });
  await dialog.getByRole('button', { name: 'Keep editing template' }).click();
  await expect(page.getByLabel('Topic', { exact: true })).toHaveValue('  Synthetic topic  ');
  await page.getByRole('button', { name: 'Write a task without a template' }).click();
  await dialog.getByRole('button', { name: 'Replace free-text draft' }).click();
  await expect(page.getByLabel('Your task')).toHaveValue(preview);
  await page.reload();
  await settle(page);
  await expect(page.getByLabel('Your task')).toHaveValue(preview);
  await page.getByRole('button', { name: 'Resume research template' }).click();
  await expect(page.getByLabel('Topic', { exact: true })).toHaveValue('  Synthetic topic  ');
  expect(writes).toEqual([]);
});

test('LOCAL a storage refusal keeps the partial template and the prior saved draft', async ({
  page,
  baseURL,
}) => {
  const { writes } = await fixture(page, baseURL!);
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/');
  await settle(page);
  await selectHomeTemplate(page, 'Write a document');
  await page.getByLabel('Subject', { exact: true }).fill('Saved subject');
  const stored = await page.evaluate(() =>
    Object.entries(sessionStorage).filter(([key]) => key.startsWith('cma:task-draft:')),
  );
  await page.evaluate(() => {
    const write = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.startsWith('cma:task-draft:'))
        throw new DOMException('Synthetic refusal', 'QuotaExceededError');
      write.call(this, key, value);
    };
  });
  await page.getByLabel('Subject', { exact: true }).fill('Keep the newer in-memory subject');
  await page.getByRole('button', { name: 'Write a task without a template' }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Switch and keep draft' }).click();
  await expect(dialog.getByRole('alert')).toContainText('could not be saved');
  await expect(dialog.getByRole('button', { name: 'Copy filled details' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Keep editing template' }).click();
  await expect(page.getByLabel('Subject', { exact: true })).toHaveValue(
    'Keep the newer in-memory subject',
  );
  expect(
    await page.evaluate(() =>
      Object.entries(sessionStorage).filter(([key]) => key.startsWith('cma:task-draft:')),
    ),
  ).toEqual(stored);
  expect(writes).toEqual([]);
});

for (const width of [1440, 390]) {
  test(`LOCAL ${width}px competing legacy drafts recover without discarding fields or starting work`, async ({
    page,
    baseURL,
  }) => {
    const { writes, boundary } = await fixture(page, baseURL!, false);
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto('/');
    await settle(page);
    await selectHomeTemplate(page, 'Write a document');
    await page.getByLabel('Document type', { exact: true }).fill(' report ');
    await page.getByLabel('Subject', { exact: true }).fill('  草稿 [subject]  ');
    await page.evaluate(() => {
      sessionStorage.setItem(
        `cma:task-draft:${JSON.stringify([location.origin, 'local-owner', null, null])}`,
        JSON.stringify({
          version: 2,
          text: '  Competing predecessor text  ',
          revision: 'predecessor-edit',
        }),
      );
    });
    await page.reload();
    await settle(page);
    await expect(page.getByLabel('Subject', { exact: true })).toHaveValue('  草稿 [subject]  ');
    await page.getByText('Other saved task drafts (1)', { exact: true }).click();
    await expect(page.getByLabel('Saved draft 1 details')).toHaveValue(
      'Task:   Competing predecessor text  \n\nShared desktop: not included',
    );
    const recover = page.getByRole('button', { name: 'Use this saved draft' });
    const stored = await page.evaluate(() =>
      Object.entries(sessionStorage).filter(([key]) => key.startsWith('cma:task-draft:')),
    );
    await page.evaluate(() => {
      const write = Storage.prototype.setItem;
      Storage.prototype.setItem = function (key, value) {
        if (key.startsWith('cma:task-draft:'))
          throw new DOMException('Synthetic refusal', 'QuotaExceededError');
        write.call(this, key, value);
      };
    });
    await recover.focus();
    await recover.press('Enter');
    await expect(page.getByRole('alert')).toContainText('could not be saved');
    await expect(recover).toBeFocused();
    await expect(page.getByLabel('Subject', { exact: true })).toHaveValue('  草稿 [subject]  ');
    expect(
      await page.evaluate(() =>
        Object.entries(sessionStorage).filter(([key]) => key.startsWith('cma:task-draft:')),
      ),
    ).toEqual(stored);
    await page.reload();
    await settle(page);
    await page.getByText('Other saved task drafts (1)', { exact: true }).click();
    await recover.click();
    await expect(page.getByLabel('Your task')).toHaveValue('  Competing predecessor text  ');
    await expect(page.getByLabel('Your task')).toBeFocused();
    await expect(page.getByLabel('Saved draft 1 details')).toHaveValue(
      'Write a document\nDocument type:  report \nSubject:   草稿 [subject]  \nAudience: \n\nShared desktop: not included',
    );
    await page.reload();
    await settle(page);
    await page.getByText('Other saved task drafts (1)', { exact: true }).click();
    await recover.click();
    await expect(page.getByLabel('Document type', { exact: true })).toBeFocused();
    await expect(page.getByLabel('Subject', { exact: true })).toHaveValue('  草稿 [subject]  ');
    await expect(page.getByLabel('Audience', { exact: true })).toHaveValue('');
    await expect(page.getByRole('button', { name: 'Start task' })).toBeDisabled();
    await page.getByLabel('Subject', { exact: true }).press('Control+Enter');
    expect(writes).toEqual([]);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
  });
}
