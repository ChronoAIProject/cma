import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

for (const width of [1440, 390]) {
  test(`LOCAL home logo, standalone tools and full-height navigation at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    if (width === 390)
      await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
    const home = page.getByRole('link', { name: 'CMA home' });
    await expect(home).toBeVisible();
    const geometry = await page.locator('.sidebar').evaluate((element) => {
      const rect = (selector: string) => element.querySelector(selector)!.getBoundingClientRect();
      const head = rect('.sidebar__head'),
        tree = rect('.resource-tree'),
        account = rect('.sidebar__account');
      const brand = rect('.sidebar__brand'),
        badge = rect('.sidebar__build'),
        create = rect('.sidebar__new'),
        toggle = rect('.sidebar__toggle');
      return {
        fills: tree.height > 600 && account.top - tree.bottom <= 12 && tree.top - head.bottom <= 12,
        separated:
          brand.right <= badge.left && badge.right <= create.left && create.right <= toggle.left,
        fits: toggle.right <= element.getBoundingClientRect().right,
      };
    });
    expect(geometry).toEqual({ fills: true, separated: true, fits: true });
    await home.click();
    await expect(page).toHaveURL(`${baseURL}/`);
    await expect(page.getByRole('heading', { name: 'Welcome to CMA' })).toBeVisible();
    await expect(page.locator('.frame__details')).toBeEmpty();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    const character = page.getByRole('button', { name: 'Spin CMA character' });
    await expect(character).toHaveAttribute('data-ready', '');
    const canvas = character.locator('canvas');
    const pixels = () =>
      canvas.evaluate(
        (element) =>
          new Promise<{ colored: number; signature: string }>((resolve) =>
            requestAnimationFrame(() => {
              const source = element as HTMLCanvasElement;
              const copy = document.createElement('canvas');
              copy.width = source.width;
              copy.height = source.height;
              const context = copy.getContext('2d')!;
              context.drawImage(source, 0, 0);
              const data = context.getImageData(0, 0, copy.width, copy.height).data;
              let colored = 0;
              for (let index = 3; index < data.length; index += 4) if (data[index]! > 0) colored++;
              resolve({ colored, signature: copy.toDataURL() });
            }),
          ),
      );
    const first = await pixels();
    expect(first.colored).toBeGreaterThan(1000);
    await expect.poll(async () => (await pixels()).signature).not.toBe(first.signature);
    await character.click();
    await page.screenshot({ path: info.outputPath(`home-${width}.png`), fullPage: true });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await expect(character).toBeDisabled();
    const still = await canvas.screenshot();
    expect(await canvas.screenshot()).toEqual(still);
    if (width === 390)
      await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
    await page.locator('.sidebar__new').click();
    await expect(page.getByRole('dialog')).toBeVisible();
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
  });

  test(`LOCAL paused and reviving hierarchy stays unavailable until ready at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'paused',
      lifecycle: { pause: { allowed: false, reason: 'not_active' }, revive: { allowed: true } },
    };
    await boundary.route(page, baseURL!);
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeEnabled();
    await expect(page.getByText(/Your work is preserved/)).toBeVisible();
    await expect(page.locator('.sandbox-page a[href^="/agents/"]')).toHaveCount(0);
    if (width === 390)
      await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
    await expect(
      page.locator('.resource-tree [aria-level="2"], .resource-tree [aria-level="3"]'),
    ).toHaveCount(0);
    if (width === 390) await page.locator('.resource-tree a').first().click();
    await page.screenshot({ path: info.outputPath(`paused-${width}.png`), fullPage: true });
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'reviving',
      lifecycle: {
        pause: { allowed: false, reason: 'not_active' },
        revive: { allowed: false, reason: 'not_paused' },
        operation: { purpose: 'revive', phase: 'home_restore', started_at: localTime },
      },
    };
    await page.reload();
    await expect(page.getByRole('status').filter({ hasText: 'Resuming' })).toBeVisible();
    await expect(page.getByText(/Current step: home restore/)).toBeVisible();
    await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeDisabled();
    await expect(page.locator('.sandbox-page a[href^="/agents/"]')).toHaveCount(0);
    await page.screenshot({ path: info.outputPath(`reviving-${width}.png`), fullPage: true });
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'active',
      lifecycle: { pause: { allowed: true }, revive: { allowed: false, reason: 'not_paused' } },
    };
    await expect(page.locator('.sandbox-page a[href^="/agents/"]')).toHaveCount(1, {
      timeout: 10000,
    });
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });

  for (const kind of ['Sandbox', 'Workspace', 'Agent'] as const) {
    test(`LOCAL ${kind} deletion keeps progress through reload and removes its row at ${width}px`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = new LocalStartup();
      boundary.agent = { ...boundary.agent, status: 'stopped' };
      boundary.workspace = {
        ...boundary.workspace,
        agents: boundary.workspace.agents.map((agent) => ({ ...agent, status: 'stopped' })),
      };
      boundary.sandbox = {
        ...boundary.sandbox,
        lifecycle: { pause: { allowed: true }, revive: { allowed: false, reason: 'not_paused' } },
      };
      await boundary.route(page, baseURL!);
      const writes: string[] = [];
      const path =
        kind === 'Sandbox'
          ? `/api/v1/sandboxes/${boundary.sandbox.id}`
          : kind === 'Workspace'
            ? `/api/v1/workspaces/${boundary.workspace.id}`
            : `/api/v1/agents/${boundary.agent.id}`;
      await page.route(`**${path}`, async (route) => {
        if (route.request().method() !== 'DELETE') return route.fallback();
        const key = route.request().headers()['idempotency-key'];
        expect(key).toBeTruthy();
        writes.push(key!);
        if (kind === 'Sandbox') boundary.sandbox = { ...boundary.sandbox, status: 'retiring' };
        if (kind === 'Workspace')
          boundary.workspace = { ...boundary.workspace, status: 'removing' };
        await route.fulfill({
          status: 202,
          contentType: 'application/json',
          body:
            kind === 'Agent'
              ? ''
              : JSON.stringify(kind === 'Sandbox' ? boundary.sandbox : boundary.workspace),
        });
      });
      await page.goto('/');
      if (width === 390)
        await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
      const level = kind === 'Sandbox' ? 1 : kind === 'Workspace' ? 2 : 3;
      const row = page.locator(`.resource-tree [aria-level="${level}"]`).first();
      await row.locator('.tree-row__menu').click();
      await page
        .getByRole('menuitem', { name: kind === 'Workspace' ? 'Remove' : 'Delete', exact: true })
        .click();
      await expect(page.getByRole('dialog', { name: `Delete ${kind}`, exact: true })).toBeVisible();
      await expect(page.getByRole('checkbox')).toBeEnabled();
      await page.getByRole('checkbox').check();
      await page.getByRole('button', { name: `Delete ${kind}`, exact: true }).click();
      await expect(
        page.getByRole('status').filter({ hasText: 'Delete request accepted' }),
      ).toBeVisible();
      expect(writes).toHaveLength(1);
      for (const dark of [false, true]) {
        await page.evaluate(
          (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
          dark,
        );
        const geometry = await page.getByRole('dialog').evaluate((element) => {
          const box = element.getBoundingClientRect();
          return {
            fits:
              box.left >= 0 && box.right <= innerWidth && box.top >= 0 && box.bottom <= innerHeight,
            overflow: element.scrollWidth > element.clientWidth + 1,
          };
        });
        expect(geometry).toEqual({ fits: true, overflow: false });
        await page.screenshot({
          path: info.outputPath(`${kind}-${width}-${dark ? 'dark' : 'light'}-deleting.png`),
          fullPage: true,
        });
      }
      await page.reload();
      await expect(
        page.getByRole('dialog', { name: `Deleting ${kind}`, exact: true }),
      ).toBeVisible();
      expect(writes).toHaveLength(1);
      if (kind === 'Sandbox') boundary.sandbox = { ...boundary.sandbox, status: 'retired' };
      if (kind === 'Workspace') boundary.workspace = { ...boundary.workspace, status: 'removed' };
      if (kind === 'Agent') {
        boundary.agent = {
          ...boundary.agent,
          status: 'deleted',
          worktree: { ...boundary.agent.worktree, status: 'removed' },
        };
        boundary.workspace = {
          ...boundary.workspace,
          agents: boundary.workspace.agents.map((agent) => ({ ...agent, status: 'deleted' })),
        };
      }
      await expect(page.getByRole('dialog', { name: `${kind} deleted`, exact: true })).toBeVisible({
        timeout: 10000,
      });
      await page.getByRole('button', { name: 'Done', exact: true }).click();
      await page.reload();
      if (width === 390)
        await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
      await expect(page.locator(`.resource-tree [aria-level="${level}"]`)).toHaveCount(0);
      expect(writes).toHaveLength(1);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
