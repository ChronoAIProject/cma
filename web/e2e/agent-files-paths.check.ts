import { createHash } from 'node:crypto';
import { test, expect, type Page, type Locator } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';

const directory = 'cma/project/.worktrees/wt-fixture';
const name = '笔记 with spaces.md';
const filePath = `${directory}/${name}`;
const text = 'Synthetic file content, distinct from its path.';
const secondEdit = `${text}\nA second edit after copying the path.`;
const digest = (value: string) => createHash('sha256').update(value).digest('hex');

async function files(page: Page) {
  const contents = new Map<string, string>();
  const writes: string[] = [];
  const reads: { operation: string; path: string }[] = [];
  await page.route('**/api/v1/agents/*/files**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.searchParams.get('path') ?? '';
    if (request.method() === 'POST') {
      const body = request.postDataJSON();
      expect(body.path).toBe(filePath);
      if (body.expected_sha256) expect(body.expected_sha256).toBe(digest(contents.get(body.path)!));
      const value = Buffer.from(body.content_base64, 'base64').toString();
      contents.set(body.path, value);
      writes.push(body.path);
      return route.fulfill({
        json: { path: body.path, size_bytes: Buffer.byteLength(value), sha256: digest(value) },
      });
    }
    expect(request.method()).toBe('GET');
    const operation = url.pathname.split('/').at(-1)!;
    reads.push({ operation, path });
    expect(path.startsWith('/')).toBe(false);
    expect(path.split('/')).not.toContain('..');
    if (operation === 'tree') {
      const entries =
        path === directory && contents.has(filePath)
          ? [
              {
                path: filePath,
                name,
                kind: 'file',
                hidden: false,
                size_bytes: Buffer.byteLength(contents.get(filePath)!),
              },
            ]
          : [];
      return route.fulfill({ json: { path, entries } });
    }
    expect(operation).toBe('content');
    expect(path).toBe(filePath);
    const value = contents.get(path)!;
    return route.fulfill({
      json: {
        path,
        size_bytes: Buffer.byteLength(value),
        sha256: digest(value),
        content_base64: Buffer.from(value).toString('base64'),
      },
    });
  });
  return { contents, writes, reads };
}

async function openComputer(page: Page) {
  await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
  const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
  await computer.getByRole('tab', { name: 'Files', exact: true }).click();
  return computer;
}

async function expectPointerTarget(button: Locator) {
  await button.scrollIntoViewIfNeeded();
  expect(
    await button.evaluate((element) => {
      const rect = element.getBoundingClientRect();
      const target = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2);
      return target !== null && element.contains(target);
    }),
    'The action must receive pointer input without an overlapping footer.',
  ).toBe(true);
}

for (const width of [390, 1600]) {
  test(`LOCAL paths stay consistent through create, preview, copy, reopen and reload at ${width}px`, async ({
    page,
    context,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = {
      ...fixture.boundary.agent,
      runtime_metadata: { working_directory: '/home/detail-fallback' },
    };
    fixture.append({
      type: 'runtime_metadata_changed',
      metadata: { working_directory: `/home/${directory}` },
    });
    try {
      await page.setViewportSize({ width, height: 1000 });
      await context.grantPermissions(['clipboard-read', 'clipboard-write']);
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
      const filesystem = await files(page);
      await page.goto('/agents/agt_contract');
      const computer = await openComputer(page);
      const current = computer.getByLabel('Current directory', { exact: true });
      await expect(current).toHaveText(`Current directory/home/${directory}`);
      await expect(computer.getByLabel('Sandbox root', { exact: true })).toHaveText(
        'Sandbox root/home/',
      );
      await expect(computer.getByLabel('Working directory', { exact: true })).toHaveText(
        `Working directory/home/${directory}`,
      );
      await expect(computer.getByLabel('Tree root', { exact: true })).toHaveText(
        `Tree root/home/${directory}`,
      );
      await computer.getByRole('button', { name: 'New file', exact: true }).click();
      const filename = computer.getByRole('textbox', { name: 'File name', exact: true });
      for (const invalid of ['../qa2.txt', 'nested/file.txt', 'nested\\file.txt', '.', '..']) {
        await filename.fill(invalid);
        await expect(computer.getByRole('button', { name: 'Create', exact: true })).toBeDisabled();
        await expect(filename).toHaveAttribute('aria-invalid', 'true');
        await expect(filename).toHaveAccessibleDescription(
          'Enter a file or folder name without path separators.',
        );
        await filename.press('Enter');
        expect(filesystem.writes).toEqual([]);
      }
      await filename.fill(name);
      await computer.getByRole('button', { name: 'Create', exact: true }).click();
      const editor = computer.getByRole('textbox', { name: `Contents of ${name}` });
      await expect(editor).toHaveValue('');
      await editor.fill(text);
      await computer.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(computer.getByText('File saved.', { exact: true })).toBeVisible();
      await computer.getByRole('button', { name, exact: true }).click();
      await expect(editor).toHaveValue(text);
      await computer.getByRole('button', { name: 'Copy path', exact: true }).click();
      await expect(computer.getByText('Path copied.', { exact: true })).toBeVisible();
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(`/home/${filePath}`);
      await expect(computer.getByLabel('Selected file', { exact: true })).toHaveText(
        `Selected file/home/${filePath}`,
      );
      expect(
        await computer
          .getByLabel('Selected file', { exact: true })
          .evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
      ).toBe(true);
      await editor.fill(secondEdit);
      const save = computer.getByRole('button', { name: 'Save', exact: true });
      await expect(save).toBeEnabled();
      await expectPointerTarget(save);
      await save.click();
      await expect.poll(() => filesystem.contents.get(filePath)).toBe(secondEdit);
      expect(filesystem.writes).toEqual([filePath, filePath, filePath]);
      const location = computer.locator('.agent-files__location');
      const preview = computer.getByRole('region', { name: 'File preview', exact: true });
      const previewBottom = await preview.evaluate(
        (element) => element.getBoundingClientRect().bottom,
      );
      const locationTop = await location.evaluate((element) => element.getBoundingClientRect().top);
      expect(locationTop).toBeGreaterThanOrEqual(previewBottom);
      const deleteFolder = location.getByRole('button', { name: 'Delete folder', exact: true });
      await expectPointerTarget(deleteFolder);
      await deleteFolder.click();
      await expect(
        computer
          .getByRole('alert')
          .filter({ hasText: `Delete /home/${directory} and everything inside it` }),
      ).toBeVisible();
      await computer.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(current).toHaveText(`Current directory/home/${directory}`);
      await page.screenshot({ path: info.outputPath(`paths-${width}.png`), fullPage: true });
      await computer.getByRole('button', { name: 'Close computer' }).click();
      await openComputer(page);
      await expect(editor).toHaveValue(secondEdit);
      await computer.getByRole('button', { name: 'Sandbox root /home/', exact: true }).click();
      await expect(current).toHaveText('Current directory/home/');
      await page.reload();
      await openComputer(page);
      await expect(current).toHaveText(`Current directory/home/${directory}`);
      await computer.getByRole('button', { name, exact: true }).click();
      await expect(editor).toHaveValue(secondEdit);
      await computer.getByRole('button', { name: 'Close computer' }).click();
      await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
      await page.getByRole('button', { name: 'Browse files' }).click();
      const dialog = page.getByRole('dialog', { name: 'Files', exact: true });
      await expect(dialog.getByLabel('Current directory', { exact: true })).toHaveText(
        `Current directory/home/${directory}`,
      );
      await dialog.getByRole('button', { name, exact: true }).click();
      await expect(dialog.getByRole('textbox', { name: `Contents of ${name}` })).toHaveValue(
        secondEdit,
      );
      await page.evaluate(() =>
        Object.defineProperty(navigator, 'clipboard', {
          configurable: true,
          value: { writeText: () => Promise.reject(new Error('Denied')) },
        }),
      );
      await dialog.getByRole('button', { name: 'Copy path', exact: true }).click();
      await expect(
        dialog.getByText('Could not copy. Select the full path above and copy it manually.'),
      ).toBeVisible();
      await expect(dialog.getByLabel('Selected file', { exact: true })).toHaveText(
        `Selected file/home/${filePath}`,
      );
      expect(filesystem.contents.get(filePath)).toBe(secondEdit);
      expect(filesystem.writes).toEqual([filePath, filePath, filePath]);
      expect(filesystem.reads).toContainEqual({ operation: 'tree', path: directory });
      expect(filesystem.reads).toContainEqual({ operation: 'tree', path: '' });
      expect(filesystem.reads).toContainEqual({ operation: 'content', path: filePath });
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
