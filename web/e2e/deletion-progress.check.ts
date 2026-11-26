import { registerDeletionStatusCases } from './redesign-deletion.cases';
import { expect, test, type Page } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';
import { LocalStartup } from './agent-startup-fixtures';

const names = [
  'Synthetic research environment with a descriptive name '.repeat(4),
  'synthetic_environment_with_a_very_long_unbroken_name_'.repeat(5),
  '合成環境の長い名前 — Synthetic environment '.repeat(6),
];
const ids = names.map((_, index) => `sbx_fixture_${index}_${'long_identifier_'.repeat(5)}`);

async function routeDeletions(page: Page, fixture: LocalAgentV2) {
  const sandboxes = names.map((name, index) => ({
    ...fixture.boundary.sandbox,
    id: ids[index]!,
    name,
    status: index === 0 ? ('active' as const) : ('retiring' as const),
    workspaces: [],
    workspace_count: 0,
    agent_count: 0,
    worktree_count: 0,
  }));
  await page.addInitScript((id) => {
    localStorage.setItem(
      'cma:deletion:local-owner',
      JSON.stringify({
        key: 'synthetic-accepted-deletion',
        accepted: true,
        action: { kind: 'delete_sandbox', sandboxId: id },
      }),
    );
  }, ids[0]!);
  await page.route('**/api/v1/sandboxes**', (route) => {
    const url = new URL(route.request().url());
    if (route.request().method() !== 'GET') return route.fallback();
    if (url.pathname === '/api/v1/sandboxes')
      return route.fulfill({ json: { items: [fixture.boundary.sandbox, ...sandboxes] } });
    const sandbox = sandboxes.find((item) => url.pathname === `/api/v1/sandboxes/${item.id}`);
    if (sandbox) return route.fulfill({ json: sandbox });
    if (sandboxes.some((item) => url.pathname === `/api/v1/sandboxes/${item.id}/events`))
      return route.fulfill({ json: [] });
    return route.fallback();
  });
}

for (const [width, theme] of [
  [1440, 'light'],
  [390, 'dark'],
] as const) {
  test(`LOCAL five deletions share one compact notice at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    const requests = Array.from({ length: 5 }, (_, index) => ({
      key: `synthetic-aggregate-${index}`,
      accepted: true,
      action: { kind: 'delete_sandbox', sandboxId: `sbx_aggregate_${index}` },
    }));
    const terminal = new Set<string>();
    try {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await fixture.route(page, baseURL!);
      await page.addInitScript((pending) => {
        for (const request of pending)
          localStorage.setItem(
            `cma:deletion-requests:local-owner:${request.key}`,
            JSON.stringify(request),
          );
      }, requests);
      await page.route('**/api/v1/sandboxes/sbx_aggregate_**', (route) => {
        if (route.request().method() !== 'GET') return route.fallback();
        const path = new URL(route.request().url()).pathname;
        if (path.endsWith('/events')) return route.fulfill({ json: [] });
        const id = path.split('/').at(-1)!;
        return route.fulfill({
          json: {
            ...fixture.boundary.sandbox,
            id,
            name: id,
            status: terminal.has(id) ? 'retired' : 'retiring',
            workspaces: [],
            workspace_count: 0,
            worktree_count: 0,
            agent_count: 0,
          },
        });
      });
      const notice = page.getByRole('region', { name: 'Deletion request', exact: true });
      const composer = page.getByRole('textbox', { name: 'Message', exact: true });
      await page.goto('/agents/agt_contract');
      await expect(composer).toBeEnabled();
      await expect(composer).toBeInViewport();
      await expect(notice).toHaveCount(0);
      await page.goto(`/sandboxes/${fixture.boundary.sandbox.id}`);
      await page.evaluate(
        (theme) => document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark'),
        theme,
      );
      await expect(notice).toHaveCount(1);
      await expect(notice).toContainText('Deleting 5 environments');
      expect((await notice.boundingBox())!.height).toBeLessThanOrEqual(72);
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true);
      await page.screenshot({ path: info.outputPath(`aggregate-${width}-${theme}.png`) });
      const open = notice.getByRole('button', { name: 'View status', exact: true });
      await open.click();
      const list = page.getByRole('dialog', { name: 'Deletion progress', exact: true });
      await expect(list.getByRole('listitem')).toHaveCount(5);
      await page.keyboard.press('Escape');
      await expect(list).toHaveCount(0);
      await expect(open).toBeFocused();
      for (const [index, request] of requests.entries()) {
        await open.click();
        await expect(list.getByText(request.action.sandboxId, { exact: true })).toBeVisible();
        terminal.add(request.action.sandboxId);
        await list
          .getByRole('listitem')
          .filter({ has: page.getByText(request.action.sandboxId, { exact: true }) })
          .getByRole('button')
          .click();
        await expect(list).toHaveCount(0);
        const completed = page.getByRole('dialog', { name: 'Sandbox deleted', exact: true });
        await expect(completed).toBeVisible();
        await completed.getByRole('button', { name: 'Done', exact: true }).click();
        const remaining = 4 - index;
        if (remaining) await expect(notice).toContainText(`Deleting ${remaining} environment`);
      }
      await expect(notice).toHaveCount(0);
      await page.goto('/agents/agt_contract');
      await expect(composer).toBeEnabled();
      await expect(composer).toBeInViewport();
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

for (const restored of [false, true]) {
  test(`LOCAL deletion completes after a 100ms reopen without reload, restored=${restored}`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    boundary.agent = { ...boundary.agent, status: 'stopped' };
    boundary.workspace = {
      ...boundary.workspace,
      agents: [{ ...boundary.workspace.agents[0]!, status: 'stopped' }],
    };
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    await boundary.route(page, baseURL!);
    const other = {
      action: { kind: 'delete_agent', agentId: 'agt_other' },
      key: 'synthetic-other-deletion',
      accepted: true,
    };
    const uncertain = {
      action: { kind: 'delete_agent', agentId: boundary.agent.id },
      key: 'synthetic-uncertain-deletion',
      accepted: false,
    };
    await page.addInitScript(
      (requests) => {
        for (const request of requests)
          localStorage.setItem(
            `cma:deletion-requests:local-owner:${request.key}`,
            JSON.stringify(request),
          );
      },
      restored ? [other, uncertain] : [other],
    );
    const agentPath = `/api/v1/agents/${boundary.agent.id}`;
    const deletes: { path: string; key?: string; body: string | null }[] = [];
    await page.route(`**${agentPath}`, (route) => {
      const request = route.request();
      if (request.method() !== 'DELETE') return route.fallback();
      deletes.push({
        path: new URL(request.url()).pathname,
        key: request.headers()['idempotency-key'],
        body: request.postData(),
      });
      return route.fulfill({ status: 202, body: '' });
    });
    const receipts = () =>
      page.evaluate(() =>
        Object.keys(localStorage)
          .filter((key) => key.startsWith('cma:deletion-requests:local-owner:'))
          .map((key) => JSON.parse(localStorage.getItem(key)!)),
      );
    // Set the clock before navigation so jumping to the anchor cannot advance an in-flight read.
    const anchor = Date.now();
    await page.clock.install({ time: anchor - 60_000 });
    await page.clock.pauseAt(anchor);
    await page.goto(`/agents/${boundary.agent.id}`);
    const moreActions = page.getByRole('button', { name: 'More Agent actions', exact: true });
    await expect(moreActions).toBeVisible();
    let submit;
    if (restored) {
      submit = page.getByRole('button', { name: 'Check request', exact: true });
    } else {
      await moreActions.click();
      await page.getByRole('menuitem', { name: /^Delete task/ }).click();
      const confirmation = page.getByRole('dialog', { name: 'Delete Agent', exact: true });
      await confirmation.getByRole('checkbox').check();
      submit = confirmation.getByRole('button', { name: 'Delete Agent', exact: true });
    }
    await expect(submit).toBeEnabled();
    expect(deletes).toEqual([]);
    const [acceptance] = await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'DELETE' &&
          new URL(response.url()).pathname === agentPath,
      ),
      submit.press('Enter'),
    ]);
    expect(acceptance.status()).toBe(202);
    const summary = page.getByRole('region', { name: 'Deletion request', exact: true });
    if (restored) {
      await expect(page.getByRole('dialog', { name: 'Deleting Agent', exact: true })).toBeHidden();
      await expect(summary).toHaveCount(0);
      await moreActions.press('Enter');
      await page
        .getByRole('menuitem', { name: 'View deletion status', exact: true })
        .press('Enter');
    } else {
      await expect(summary.getByRole('button', { name: 'View status', exact: true })).toBeEnabled();
      await summary.getByRole('button', { name: 'View status', exact: true }).press('Enter');
    }
    const reopen = page.getByRole('button', { name: 'View Agent deletion status (2)' });
    await expect(reopen).toBeEnabled();
    const original = (await receipts()).find(
      (request) => request.action.agentId === boundary.agent.id,
    );
    expect(original).toEqual({ ...uncertain, key: expect.any(String), accepted: true });
    expect(original.key).toBeTruthy();
    if (restored) expect(original.key).toBe(uncertain.key);
    await page.clock.runFor(100);
    await reopen.press('Enter');
    expect(await page.evaluate(() => Date.now())).toBe(anchor + 100);
    await expect(page.getByRole('dialog', { name: 'Deleting Agent', exact: true })).toBeVisible();
    boundary.agent = {
      ...boundary.agent,
      status: 'deleted',
      worktree: { ...boundary.agent.worktree, status: 'removed' },
    };
    boundary.workspace = { ...boundary.workspace, agents: [] };
    await expect
      .poll(async () => {
        await page.clock.runFor(250);
        return page.getByRole('dialog', { name: 'Agent deleted', exact: true }).isVisible();
      })
      .toBe(true);
    expect(await receipts()).toEqual([other]);
    expect(deletes).toEqual([{ path: agentPath, key: original.key, body: null }]);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}

for (const theme of ['light', 'dark']) {
  for (const [width, zoom] of [
    [1280, 1],
    [390, 1],
    [1280, 2],
  ] as const) {
    test(`LOCAL deletion progress rows fit ${width}px at ${zoom * 100}% zoom in ${theme}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      const writes: string[] = [];
      page.on('request', (request) => {
        if (
          new URL(request.url()).pathname.startsWith('/api/') &&
          !['GET', 'HEAD'].includes(request.method())
        )
          writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
      });
      try {
        await page.setViewportSize({ width, height: 1000 });
        await fixture.route(page, baseURL!);
        await routeDeletions(page, fixture);
        await page.goto('/agents/agt_contract');
        await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeEnabled();
        await page.evaluate(
          ({ theme, zoom }) => {
            document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark');
            document.documentElement.style.zoom = String(zoom);
            Object.defineProperty(navigator, 'clipboard', {
              configurable: true,
              value: {
                writeText: async (value: string) => {
                  sessionStorage.setItem('synthetic-copied-id', value);
                },
              },
            });
          },
          { theme, zoom },
        );
        const sidebar = page.getByRole('complementary', {
          name: 'Application navigation',
          exact: true,
          includeHidden: true,
        });
        if (await sidebar.evaluate((element) => element.classList.contains('sidebar--collapsed')))
          await page.getByRole('button', { name: 'Show sidebar', exact: true }).click();
        const summary = sidebar.getByText('Cleaning up 3 sandboxes', { exact: true });
        await summary.focus();
        await page.keyboard.press('Enter');
        const progress = sidebar.locator('details').filter({ hasText: 'Cleaning up 3 sandboxes' });
        for (const name of names) {
          const label = progress.getByText(name, { exact: true });
          await expect(label).toBeVisible();
          expect(
            await label.evaluate((element) => {
              const bounds = element.getBoundingClientRect();
              const range = document.createRange();
              range.selectNodeContents(element);
              const text = range.getBoundingClientRect();
              return text.top >= bounds.top - 1 && text.bottom <= bounds.bottom + 1;
            }),
          ).toBe(true);
        }
        const rows = progress.getByRole('listitem');
        await expect(rows).toHaveCount(3);
        const geometry = async () =>
          rows.evaluateAll((elements) =>
            elements.every((element, index) => {
              const row = element.getBoundingClientRect();
              const previous = elements[index - 1]?.getBoundingClientRect();
              const container = element.closest('details')!.getBoundingClientRect();
              return (
                element.scrollWidth <= element.clientWidth + 1 &&
                row.left >= container.left &&
                row.right <= container.right + 1 &&
                (previous === undefined || previous.bottom < row.top)
              );
            }),
          );
        expect(await geometry()).toBe(true);
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await expect(rows.nth(0)).toContainText('Deletion requested');
        await expect(rows.nth(1)).toContainText('Deleting');
        await page.keyboard.press('Tab');
        const view = rows.first().getByRole('button', { name: `View status for ${names[0]}` });
        await expect(view).toBeFocused();
        await page.keyboard.press('Tab');
        await expect(
          rows.first().getByRole('button', { name: `Copy identifier for ${names[0]}` }),
        ).toBeFocused();
        await page.keyboard.press('Enter');
        expect(await page.evaluate(() => sessionStorage.getItem('synthetic-copied-id'))).toBe(
          ids[0],
        );
        await expect(rows.first().getByRole('status')).toHaveText('Identifier copied.');
        await page.evaluate(() =>
          Object.defineProperty(navigator, 'clipboard', {
            configurable: true,
            value: {
              writeText: async () => {
                throw new DOMException('Synthetic denial', 'NotAllowedError');
              },
            },
          }),
        );
        await rows
          .nth(1)
          .getByRole('button', { name: `Copy identifier for ${names[1]}` })
          .click();
        await expect(rows.nth(1).getByRole('status')).toHaveText(
          'Copy unavailable. Select the full identifier below.',
        );
        await expect(rows.nth(1).locator('.deletion-progress__full')).toHaveText(ids[1]!);
        expect(await geometry()).toBe(true);
        await page.screenshot({ path: info.outputPath(`deletion-${width}-${zoom}-${theme}.png`) });
        await view.focus();
        await page.keyboard.press('Enter');
        await expect(
          page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true }),
        ).toBeVisible();
        expect(writes).toEqual([]);
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

registerDeletionStatusCases();
