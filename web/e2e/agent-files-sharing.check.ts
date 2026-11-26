import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import {
  test,
  expect,
  type Download,
  type Locator,
  type Page,
  type TestInfo,
} from '@playwright/test';
import type { AgentShareLink } from '../src/api-client/index';
import { LocalStartup, localTime } from './agent-startup-fixtures';

// Public HTTP fixtures exercise the full browser UI, not a running Sandbox/backend.
const longName = 'research-findings-with-a-deliberately-long-filename-for-truncation.md';
const digest = (text: string) => createHash('sha256').update(text).digest('hex');

function hold() {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

async function files(page: Page, writable = true) {
  await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
    route.fulfill({ status: 404, json: { code: 'not_found' } }),
  );
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
  await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
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
  await fileDialog.getByRole('button', { name: 'Sandbox root /home/', exact: true }).click();
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
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await dialog.getByRole('button', { name: 'Copy path', exact: true }).click();
  await expect(dialog.getByText('Path copied.', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('/home/.dockerignore');
  await screenshots(page, dialog, 'shared-files', info);
  expect(filesystem.writes).toEqual([]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

for (const { access, unavailable } of [
  { access: 'owner', unavailable: false },
  { access: 'reader', unavailable: false },
  { access: 'writer', unavailable: false },
  { access: 'writer', unavailable: true },
]) {
  test(`LOCAL ${access} Computer retains the Files selection during ${unavailable ? 'failed' : 'delayed'} initialization`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    const filesystem = await files(page, access !== 'reader');
    let accessUnavailable = false;
    let sharedTitle = 'Shared Agent';
    let revalidateAt: string | null = null;
    let revalidation: Promise<void> | undefined;
    if (access === 'reader') {
      await page.clock.install({ time: new Date(localTime) });
      revalidateAt = new Date(Date.parse(localTime) + 30_000).toISOString();
    }
    if (access !== 'owner') {
      await page.route('**/api/v1/agents/agt_contract', (route) =>
        route.fulfill({
          json: {
            access: 'shared',
            id: 'agt_contract',
            title: sharedTitle,
            status: 'ready',
            created_at: localTime,
            updated_at: localTime,
          },
        }),
      );
      await page.route('**/api/v1/resource-access/agent/agt_contract', (route) =>
        route.fulfill(
          accessUnavailable
            ? { status: 503, json: { code: 'temporarily_unavailable' } }
            : {
                json: {
                  capabilities: [
                    'view_details',
                    'view_history',
                    'view_files',
                    ...(access === 'writer' ? ['operate_agent'] : []),
                  ],
                  policy_revision: 1,
                  revalidate_at: null,
                  owner: { kind: 'person', is_current_person: false },
                },
              },
        ),
      );
    }
    let started!: () => void;
    let complete!: () => void;
    const pending = new Promise<void>((resolve) => {
      started = resolve;
    });
    const completion = new Promise<void>((resolve) => {
      complete = resolve;
    });
    await page.route('**/api/v1/agents/agt_contract/environment', async (route) => {
      started();
      await completion;
      await revalidation;
      if (unavailable) {
        return route.fulfill({
          status: 503,
          headers: { 'content-type': 'application/problem+json' },
          json: {
            type: 'about:blank',
            title: 'Unavailable',
            status: 503,
            code: 'temporarily_unavailable',
          },
        });
      }
      await route.fulfill({
        json: {
          agent_id: 'agt_contract',
          policy_revision: 1,
          runtime_incarnation: null,
          revalidate_at: revalidateAt,
          files: {
            state: 'available',
            reason: 'authorized_mounted_home',
            scope: 'mounted_home',
            writable: access !== 'reader',
          },
          operation_output: {
            state: 'available',
            reason: 'recorded_output_only',
            scope: 'agent_turn_history',
            writable: false,
          },
          browser: {
            state: 'not_enabled',
            reason: 'browser_disabled',
            scope: 'agent_runtime',
            writable: false,
          },
          replay: {
            state: 'unsupported',
            reason: 'not_supported_by_contract',
            scope: 'none',
            writable: false,
          },
          control: {
            state: 'unsupported',
            reason: 'not_supported_by_contract',
            scope: 'none',
            writable: false,
          },
        },
      });
    });
    await page.setViewportSize({ width: 1440, height: 1000 });
    try {
      await page.goto('/agents/agt_contract');
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      await pending;
      const computer = page.getByRole('complementary', { name: 'Agent computer' });
      const filesTab = computer.getByRole('tab', { name: 'Files' });
      await filesTab.click();
      await expect(filesTab).toHaveAttribute('aria-selected', 'true');
      await expect(computer.getByText('Checking file access…')).toBeVisible();
      complete();
      if (unavailable) {
        await expect(computer.getByText('We couldn’t check file availability.')).toBeVisible();
      } else {
        if (access !== 'owner')
          await expect(computer.getByRole('button', { name: 'README.md' })).toBeVisible();
        const newFile = computer.getByRole('button', { name: 'New file', exact: true });
        if (access === 'reader') await expect(newFile).toHaveCount(0);
        else await expect(newFile).toBeEnabled();
        // The explicit child navigation survives a real browser access refresh.
        await computer.getByRole('button', { name: 'Sandbox root /home/' }).click();
        await computer.getByRole('button', { name: 'cma', exact: true }).click();
        for (const width of [1440, 375]) {
          await page.setViewportSize({ width, height: 812 });
          await expect(computer.getByLabel('Current directory')).toContainText('/home/cma');
          await expect(
            computer.getByRole('button', { name: 'notes.md', exact: true }),
          ).toBeVisible();
          sharedTitle = `Shared Agent checked at ${width}px`;
          await page.evaluate(() => window.dispatchEvent(new Event('focus')));
          if (access !== 'owner') {
            // Observe the published access result before sending another focus event.
            // Foreground events coalesce while an access read is pending.
            await expect(
              page.getByRole('heading', { name: sharedTitle, exact: true }),
            ).toBeAttached();
          }
          await expect(computer.getByLabel('Current directory')).toContainText('/home/cma');
        }
        if (access !== 'owner') {
          accessUnavailable = true;
          await page.evaluate(() => window.dispatchEvent(new Event('focus')));
          const retry = page.getByRole('button', { name: 'Check access again', exact: true });
          await expect(page.getByRole('heading', { name: 'Agent unavailable' })).toBeVisible();
          accessUnavailable = false;
          await retry.click();
          await expect(computer.getByLabel('Current directory')).toContainText('/home/cma');
          await expect(
            computer.getByRole('button', { name: 'notes.md', exact: true }),
          ).toBeVisible();
        }
        await page.setViewportSize({ width: 1440, height: 1000 });
        await computer.getByRole('button', { name: 'Sandbox root /home/', exact: true }).click();
        await computer.getByRole('checkbox', { name: 'Show hidden' }).check();
        await computer.getByRole('button', { name: '.dockerignore', exact: true }).click();
        const preview = computer.getByRole('textbox', { name: 'Contents of .dockerignore' });
        const dotfile = filesystem.contents.get('.dockerignore')!;
        await expect(preview).toHaveValue(dotfile);
        const downloadButton = computer.getByRole('button', { name: 'Download file' });
        const downloadPath = '**/agt_contract/files/download?path=.dockerignore';
        const downloadDotfile = async () => {
          const [download] = await Promise.all([
            page.waitForEvent('download', { timeout: 5_000 }),
            downloadButton.click(),
          ]);
          expect(download.suggestedFilename()).toMatch(/^\.?dockerignore$/);
          expect(await readFile((await download.path())!, 'utf8')).toBe(dotfile);
        };
        await downloadDotfile();
        if (access === 'reader') {
          const holdDownload = hold();
          const holdAccess = hold();
          await page.route(
            downloadPath,
            async (route) => {
              await holdDownload.promise;
              await route.abort();
            },
            { times: 1 },
          );
          try {
            await Promise.all([page.waitForRequest(downloadPath), downloadButton.click()]);
            await expect(computer.getByText('Downloading file…')).toBeVisible();
            revalidateAt = null;
            revalidation = holdAccess.promise;
            await page.clock.fastForward(31_000);
            await expect(computer.getByText('Checking file access…')).toBeVisible();
            holdAccess.release();
            const interrupted = 'Download was interrupted. Try downloading the file again.';
            await expect(computer.getByRole('alert')).toContainText(interrupted);
          } finally {
            holdAccess.release();
            holdDownload.release();
          }
          await downloadDotfile();
          const heldResponse = hold();
          const lateResponse = hold();
          const downloadEvents: string[] = [];
          const recordDownload = (download: Download) =>
            downloadEvents.push(download.suggestedFilename());
          page.on('download', recordDownload);
          await page.route(
            downloadPath,
            async (route) => {
              await heldResponse.promise;
              await route.fulfill({ contentType: 'application/octet-stream', body: dotfile });
              lateResponse.release();
            },
            { times: 1 },
          );
          const interrupted = 'Download was interrupted. Try downloading the file again.';
          const notice = page.getByRole('alert').filter({ hasText: interrupted });
          try {
            const [request] = await Promise.all([
              page.waitForRequest(downloadPath),
              downloadButton.click(),
            ]);
            await expect(computer.getByText('Downloading file…')).toBeVisible();
            const failed = page.waitForEvent('requestfailed', {
              predicate: (item) => item === request,
            });
            accessUnavailable = true;
            await page.evaluate(() => window.dispatchEvent(new Event('focus')));
            await expect(page.getByRole('heading', { name: 'Agent unavailable' })).toBeVisible();
            await expect(computer).toHaveCount(0);
            expect((await failed).failure()?.errorText).toContain('ABORTED');
            await expect(notice).toBeVisible();
            await expect(notice.getByRole('button', { name: 'Dismiss' })).toBeEnabled();
            expect(downloadEvents).toEqual([]);
            accessUnavailable = false;
            await page.getByRole('button', { name: 'Check access again', exact: true }).click();
            await computer.getByRole('checkbox', { name: 'Show hidden' }).check();
            await computer.getByRole('button', { name: '.dockerignore', exact: true }).click();
            await expect(preview).toHaveValue(dotfile);
            await downloadDotfile();
            await expect(notice).toHaveCount(0);
            heldResponse.release();
            await lateResponse.promise;
            await expect(notice).toHaveCount(0);
            expect(downloadEvents).toHaveLength(1);
          } finally {
            heldResponse.release();
            page.off('download', recordDownload);
          }
          await page.route(
            downloadPath,
            (route) => route.fulfill({ status: 404, json: { code: 'resource_not_found' } }),
            { times: 1 },
          );
          const unexpectedDownloads: string[] = [];
          page.on('download', (download) => unexpectedDownloads.push(download.suggestedFilename()));
          await downloadButton.click();
          const denied =
            'File access is no longer available. Ask the Agent owner to check your access.';
          await expect(page.getByRole('alert').filter({ hasText: denied })).toBeVisible();
          await expect(preview).toHaveCount(0);
          expect(unexpectedDownloads).toEqual([]);
        }
      }
      await expect(filesTab).toHaveAttribute('aria-selected', 'true');
      await expect(computer.getByRole('tab', { name: 'Screen' })).toHaveAttribute(
        'aria-selected',
        'false',
      );
      expect(filesystem.writes).toEqual([]);
      expect(boundary.writes).toEqual([]);
    } finally {
      complete();
    }
  });
}
