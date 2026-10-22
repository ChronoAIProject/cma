import { createHash } from 'node:crypto';
import { test, expect, type Locator, type Page, type TestInfo } from '@playwright/test';
import type { AgentShareLink } from '../../sdk/typescript/src/index';
import { LocalStartup, localTime } from './agent-startup-fixtures';

// Public HTTP fixtures exercise the full browser UI, not a running Sandbox/backend.
const longName = 'research-findings-with-a-deliberately-long-filename-for-truncation.md';
const digest = (text: string) => createHash('sha256').update(text).digest('hex');

async function files(page: Page, writable = true) {
  const contents = new Map([
    ['README.md', '# Research workspace\n\nFiles are persisted under /home.\n'],
    [longName, '# Findings\nA deliberately long filename should stay on one row.\n'],
    ['.dockerignore', 'node_modules\ntarget\n'],
    ['cma/notes.md', 'A nested workspace note.\n'],
  ]);
  const writes: { method: string; path: string }[] = [];
  await page.route('**/api/v1/agents/agt_contract/files**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.searchParams.get('path') ?? '';
    if (request.method() === 'POST') {
      expect(writable).toBe(true);
      const body = request.postDataJSON();
      if (body.expected_sha256) expect(body.expected_sha256).toBe(digest(contents.get(body.path)!));
      const text = Buffer.from(body.content_base64, 'base64').toString();
      contents.set(body.path, text);
      writes.push({ method: 'POST', path: body.path });
      return route.fulfill({
        json: { path: body.path, size_bytes: Buffer.byteLength(text), sha256: digest(text) },
      });
    }
    if (request.method() === 'DELETE') {
      expect(writable).toBe(true);
      contents.delete(path);
      writes.push({ method: 'DELETE', path });
      return route.fulfill({ json: { path, deleted: true } });
    }
    if (url.pathname.endsWith('/tree')) {
      const prefix = path ? `${path}/` : '';
      const entries = [...contents].flatMap(([name, text]) => {
        if (!name.startsWith(prefix) || name.slice(prefix.length).includes('/')) return [];
        const leaf = name.slice(prefix.length);
        return [
          {
            path: name,
            name: leaf,
            kind: 'file',
            size_bytes: Buffer.byteLength(text),
            hidden: leaf.startsWith('.'),
          },
        ];
      });
      if (!path)
        entries.unshift({
          path: 'cma',
          name: 'cma',
          kind: 'directory',
          size_bytes: 0,
          hidden: false,
        });
      return route.fulfill({ json: { path, entries } });
    }
    const text = contents.get(path)!;
    if (url.pathname.endsWith('/download'))
      return route.fulfill({ contentType: 'application/octet-stream', body: text });
    return route.fulfill({
      json: {
        path,
        size_bytes: Buffer.byteLength(text),
        sha256: digest(text),
        content_base64: Buffer.from(text).toString('base64'),
      },
    });
  });
  return { contents, writes };
}

async function details(page: Page) {
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
}

async function screenshots(page: Page, dialog: Locator, name: string, info: TestInfo) {
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await dialog.getByRole('heading').first().scrollIntoViewIfNeeded();
      await expect
        .poll(() =>
          dialog.evaluate((element) => {
            const box = element.getBoundingClientRect();
            return (
              box.left >= 0 &&
              box.right <= innerWidth + 1 &&
              box.top >= 0 &&
              box.bottom <= innerHeight + 1 &&
              element.scrollWidth <= element.clientWidth + 1
            );
          }),
        )
        .toBe(true);
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`${name}-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
      const lowerContent =
        name === 'share-links' ? dialog.getByRole('article').last() : dialog.getByRole('textbox');
      await lowerContent.scrollIntoViewIfNeeded();
      await expect(lowerContent).toBeVisible();
      await expect
        .poll(() =>
          lowerContent.evaluate((element) => {
            const box = element.getBoundingClientRect();
            return (
              box.left >= 0 &&
              box.right <= innerWidth + 1 &&
              element.scrollWidth <= element.clientWidth + 1
            );
          }),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`${name}-content-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
}

test('LOCAL owner files and independent share links fit desktop and narrow screens', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const filesystem = await files(page);
  let links: AgentShareLink[] = [];
  const commands: { method: string; key?: string }[] = [];
  await page.route('**/api/v1/organizations', (route) =>
    route.fulfill({
      json: { items: [{ id: 'research', role: 'member', display_name: 'Research team' }] },
    }),
  );
  await page.route('**/api/v1/agents/agt_contract/share-links**', async (route) => {
    const request = route.request();
    if (request.method() === 'GET') return route.fulfill({ json: { items: links } });
    commands.push({ method: request.method(), key: request.headers()['idempotency-key'] });
    if (request.method() === 'POST') {
      const body = request.postDataJSON();
      expect(body.token).toMatch(/^[a-f0-9]{64}$/);
      const link: AgentShareLink = {
        id: `share-${links.length + 1}`,
        agent_id: 'agt_contract',
        audience: body.audience,
        mode: body.mode,
        shared_by: { subject: 'local-owner', display_name: 'Alex Chen', avatar_url: null },
        created_at: localTime,
        revoked_at: null,
        revision: 1,
        accepted_count: 0,
      };
      links.push(link);
      return route.fulfill({ json: link });
    }
    const url = new URL(request.url());
    const link = links.find((item) => url.pathname.endsWith(`/${item.id}`))!;
    expect(url.searchParams.get('expected_revision')).toBe(String(link.revision));
    links = links.filter((item) => item.id !== link.id);
    return route.fulfill({ json: { ...link, revision: 2, revoked_at: localTime } });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await composer.fill('Preserve my unsent message while browsing files.');
  await details(page);
  await page.getByRole('button', { name: 'Browse files' }).click();
  const fileDialog = page.getByRole('dialog', { name: 'Files', exact: true });
  await expect(fileDialog.getByRole('button', { name: '.dockerignore', exact: true })).toHaveCount(
    0,
  );
  await fileDialog.getByRole('checkbox', { name: 'Show hidden' }).check();
  await fileDialog.getByRole('button', { name: '.dockerignore', exact: true }).click();
  await fileDialog
    .getByRole('textbox', { name: 'Contents of .dockerignore' })
    .fill('node_modules\ntarget\n.cache\n');
  await fileDialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(fileDialog.getByText('File saved.')).toBeVisible();
  expect(filesystem.contents.get('.dockerignore')).toBe('node_modules\ntarget\n.cache\n');
  await fileDialog.getByRole('button', { name: longName, exact: true }).click();
  await expect(fileDialog.getByRole('textbox', { name: `Contents of ${longName}` })).toContainText(
    'A deliberately long filename',
  );
  await screenshots(page, fileDialog, 'files', info);
  expect(
    await fileDialog
      .locator('.agent-files__row')
      .evaluateAll((rows) => rows.every((row) => row.getBoundingClientRect().height === 32)),
  ).toBe(true);
  const download = page.waitForEvent('download');
  await fileDialog.getByRole('button', { name: 'Download file' }).click();
  expect((await download).suggestedFilename()).toBe(longName);
  await fileDialog.getByRole('button', { name: 'Close', exact: true }).click();
  await page.getByRole('button', { name: 'Manage share links' }).click();
  const shareDialog = page.getByRole('dialog', { name: 'Share Agent session' });
  await shareDialog.getByRole('button', { name: 'Create link' }).click();
  await expect(shareDialog.getByRole('textbox', { name: 'Share link 1 URL' })).toHaveValue(
    /\/agent-manager\/share\/share-1#[a-f0-9]{64}$/,
  );
  await shareDialog.getByRole('button', { name: 'Share audience' }).click();
  await page.getByRole('menuitemradio', { name: 'Organization members' }).click();
  await shareDialog.getByRole('button', { name: 'Share permission' }).click();
  await page.getByRole('menuitemradio', { name: 'View and write', exact: true }).click();
  await shareDialog.getByRole('button', { name: 'Create link' }).click();
  await expect(shareDialog.getByRole('textbox', { name: 'Share link 2 URL' })).toBeVisible();
  await screenshots(page, shareDialog, 'share-links', info);
  const firstLink = shareDialog.getByRole('article', { name: 'Share link 1', exact: true });
  await firstLink.getByRole('button', { name: 'Revoke link' }).click();
  await firstLink.getByRole('button', { name: 'Revoke access' }).click();
  await expect(shareDialog.getByRole('article')).toHaveCount(1);
  expect(links.map((link) => link.id)).toEqual(['share-2']);
  await shareDialog.getByRole('button', { name: 'Close', exact: true }).click();
  await page.getByRole('button', { name: 'Close Agent details' }).click();
  await expect(composer).toHaveValue('Preserve my unsent message while browsing files.');
  expect(commands).toHaveLength(3);
  expect(commands.every((command) => Boolean(command.key))).toBe(true);
  expect(filesystem.writes).toEqual([{ method: 'POST', path: '.dockerignore' }]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

test('LOCAL share URL redemption opens an attributed reader with read-only files', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const filesystem = await files(page, false);
  const token = 'ab'.repeat(32);
  const redemption: unknown[] = [];
  const shared = {
    access: 'shared',
    id: 'agt_contract',
    title: 'Shared research session',
    status: 'ready',
    created_at: localTime,
    updated_at: localTime,
  };
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'local-reader',
        display_name: 'Local reader',
        capabilities: ['use_own_sandboxes'],
      },
    }),
  );
  await page.route('**/api/v1/sandboxes', (route) => route.fulfill({ json: { items: [] } }));
  await page.route('**/api/v1/agents/agt_contract', (route) => route.fulfill({ json: shared }));
  await page.route('**/api/v1/agents/provenance**', (route) =>
    route.fulfill({
      json: {
        items: [
          {
            agent_id: shared.id,
            title: shared.title,
            status: shared.status,
            created_at: localTime,
            updated_at: localTime,
            creation_origin: 'manual',
          },
        ],
      },
    }),
  );
  await page.route('**/api/v1/resource-access/agent/agt_contract', (route) =>
    route.fulfill({
      json: {
        capabilities: ['view_details', 'view_history', 'view_files'],
        policy_revision: 1,
        revalidate_at: null,
        owner: { kind: 'person', is_current_person: false },
        shared_by: { subject: 'local-owner', display_name: 'Alex Chen', avatar_url: null },
      },
    }),
  );
  await page.route('**/api/v1/agent-share-links/shared-link/redeem', (route) => {
    expect(route.request().url()).not.toContain(token);
    redemption.push(route.request().postDataJSON());
    return route.fulfill({
      json: {
        id: 'shared-link',
        agent_id: 'agt_contract',
        audience: { kind: 'public' },
        mode: 'view',
        shared_by: { subject: 'local-owner', display_name: 'Alex Chen', avatar_url: null },
        created_at: localTime,
        revoked_at: null,
        revision: 2,
        accepted_count: 1,
      },
    });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(`/agent-manager/share/shared-link#${token}`);
  await expect(page.getByRole('heading', { name: shared.title })).toBeVisible();
  await expect(page.getByRole('note')).toContainText('Shared by Alex Chen');
  await expect(page.getByRole('note')).toContainText('View-only access');
  expect(new URL(page.url()).hash).toBe('');
  expect(redemption.length).toBeGreaterThan(0);
  expect(redemption.every((body) => JSON.stringify(body) === JSON.stringify({ token }))).toBe(true);
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Permission mode' })).toHaveCount(0);
  await page.screenshot({
    path: info.outputPath('shared-reader-desktop.png'),
    fullPage: true,
    animations: 'disabled',
  });
  await details(page);
  await page.getByRole('button', { name: 'Browse files' }).click();
  const dialog = page.getByRole('dialog', { name: 'Files', exact: true });
  await dialog.getByRole('checkbox', { name: 'Show hidden' }).check();
  await dialog.getByRole('button', { name: '.dockerignore', exact: true }).click();
  await expect(dialog.getByRole('textbox', { name: 'Contents of .dockerignore' })).toHaveAttribute(
    'readonly',
    '',
  );
  await expect(dialog.getByRole('button', { name: 'Download file' })).toBeEnabled();
  await expect(dialog.getByRole('button', { name: 'Delete file' })).toHaveCount(0);
  await expect(dialog.getByRole('button', { name: 'Upload' })).toHaveCount(0);
  await screenshots(page, dialog, 'shared-files', info);
  expect(filesystem.writes).toEqual([]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
