import { test, expect } from '@playwright/test';
import type { Launch } from '../../sdk/typescript/src/index';
import { LocalStartup, localTime, recorded } from './agent-startup-fixtures';

for (const viewport of [
  { width: 1440, height: 900 },
  { width: 800, height: 600 },
  { width: 390, height: 844 },
  { width: 320, height: 480 },
  { width: 320, height: 240 },
  { width: 900, height: 240 },
]) {
  test(`LOCAL modal controls remain reachable at ${viewport.width}x${viewport.height}`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.goto('/agent-manager');
    await page.getByRole('button', { name: 'New agent', exact: true }).first().click();
    await page.setViewportSize(viewport);
    const dialog = page.getByRole('dialog', { name: 'New agent', exact: true });
    const geometry = await dialog.evaluate((element) => {
      const box = element.getBoundingClientRect();
      const body = element.querySelector<HTMLElement>('.modal__body')!;
      return {
        margins: [
          box.left / innerWidth,
          (innerWidth - box.right) / innerWidth,
          box.top / innerHeight,
          (innerHeight - box.bottom) / innerHeight,
        ],
        centerX: (box.left + box.right) / 2,
        centerY: (box.top + box.bottom) / 2,
        bodyHeight: body.clientHeight,
        horizontalOverflow: element.scrollWidth > element.clientWidth + 1,
        overflow: [...element.querySelectorAll<HTMLElement>('*')]
          .filter((child) => child.getBoundingClientRect().right > box.right + 1)
          .map((child) => ({ name: child.className, width: child.clientWidth })),
      };
    });
    expect(geometry.margins.every((margin) => margin >= 0.2 - 0.001)).toBe(true);
    expect(geometry.centerX).toBeCloseTo(viewport.width / 2, 0);
    expect(geometry.centerY).toBeCloseTo(viewport.height / 2, 0);
    expect(geometry.bodyHeight).toBeGreaterThan(0);
    expect(geometry.horizontalOverflow, JSON.stringify(geometry)).toBe(false);

    const repository = dialog.getByLabel('GitHub repository (optional)');
    await repository.fill('https://github.com/example/repository');
    // Filling can focus a clipped input. A real click must also survive clipping and hit testing.
    await repository.click();
    await expect(repository).toHaveValue('https://github.com/example/repository');
    await repository.fill('');
    const create = dialog.getByRole('button', { name: 'Create agent', exact: true });
    await create.click({ trial: true });
    await page.screenshot({
      path: info.outputPath(`modal-${viewport.width}x${viewport.height}-footer.png`),
      animations: 'disabled',
    });
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(dialog).toHaveCount(0);

    // Reopen to exercise the other footer action after scrolling through the body.
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.getByRole('button', { name: 'New agent', exact: true }).first().click();
    await page.setViewportSize(viewport);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
    await info.attach('modal-geometry', {
      body: JSON.stringify({ viewport, ...geometry }, null, 2),
      contentType: 'application/json',
    });
  });
}

for (const width of [1440, 390]) {
  for (const surface of ['launch', 'sandbox'] as const) {
    test(`LOCAL live restore progress on ${surface} at ${width}px`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = new LocalStartup();
      const startedAt = new Date(Date.now() - 185_000).toISOString();
      const operation = {
        purpose: 'revive' as const,
        phase: 'home_restore' as const,
        started_at: startedAt,
      };
      boundary.sandbox = {
        ...boundary.sandbox,
        status: 'reviving',
        lifecycle: { pause: { allowed: false }, revive: { allowed: false }, operation },
      };
      await boundary.route(page, baseURL!);
      const original = recorded<Launch>('launches-state.json', 'launch-state-repeat-new');
      const record: Launch = {
        ...original,
        steps: original.steps.map((step) =>
          step.key === 'sandbox' ? { ...step, status: 'running' } : step,
        ),
        state: 'sandbox_pending',
        blocked_reason: { type: 'sandbox_paused_reviving' },
        resolution: {
          sandbox: { state: 'reused', id: boundary.sandbox.id },
          workspace: { state: 'pending' },
          agent: { state: 'pending' },
          choice_required: false,
        },
      };
      await page.route('**/api/v1/launches/lch_contract', (route) =>
        route.fulfill({ json: record }),
      );
      await page.route('**/api/v1/agui/launches/**', (route) =>
        route.fulfill({ status: 503, json: { title: 'Stream unavailable' } }),
      );
      await page.goto(
        surface === 'launch' ? '/launches/lch_contract' : `/sandboxes/${boundary.sandbox.id}`,
      );
      const progress = page.getByRole('region', { name: 'Sandbox progress' });
      await expect(progress).toBeVisible();
      await expect(progress.getByText(/Elapsed: 3m/)).toBeVisible();
      await expect(progress.getByText(/File-level progress is not reported yet/)).toBeVisible();
      await progress.getByText('Activity details').click();
      await expect(progress.getByText('Done', { exact: true })).toHaveCount(3);
      for (const dark of [false, true]) {
        await page.evaluate(
          (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
          dark,
        );
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        await page.screenshot({
          path: info.outputPath(`restore-${surface}-${width}-${dark ? 'dark' : 'light'}.png`),
          fullPage: true,
        });
      }
      let failReads = true;
      await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}`, (route) =>
        failReads
          ? route.fulfill({ status: 503, json: { title: 'Temporarily unavailable' } })
          : route.fulfill({ json: boundary.sandbox }),
      );
      await expect(progress.getByText(/Status updates interrupted/)).toBeVisible({
        timeout: 10_000,
      });
      await expect(progress.getByText('Last confirmed stage', { exact: true })).toBeVisible();
      failReads = false;
      boundary.sandbox = {
        ...boundary.sandbox,
        lifecycle: {
          ...boundary.sandbox.lifecycle!,
          operation: { ...operation, phase: 'conversation_reopen' },
        },
      };
      await expect(progress.getByText('Done', { exact: true })).toHaveCount(4, { timeout: 10_000 });
      await expect(progress.getByText(/Status updates interrupted/)).toHaveCount(0);
      await page.reload();
      await expect(progress).toBeVisible();
      await expect(progress.getByText(/Elapsed: 3m/)).toBeVisible();
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}

for (const width of [1440, 390]) {
  test(`LOCAL signed-out lens invitation and isolated sign-in handover at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.route('**/auth/session', (route) =>
      route.fulfill({
        contentType: 'application/json',
        body: JSON.stringify({ authenticated: false }),
      }),
    );
    let signIns = 0;
    await page.route('**/auth/login?*', (route) => {
      signIns++;
      return route.fulfill({ contentType: 'text/html', body: '<h1>Identity sign-in</h1>' });
    });
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'ChronoAI Managed Agent' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toHaveCount(0);
    const logo = page.getByRole('button', { name: 'Get Started', exact: true });
    await expect(logo).toHaveAttribute('data-ready', '');
    await expect(logo).toHaveAttribute('data-invitation', '', { timeout: 15000 });
    const canvas = logo.locator('canvas');
    const nonblank = await canvas.evaluate(
      (element) =>
        new Promise<number>((resolve) =>
          requestAnimationFrame(() => {
            const source = element as HTMLCanvasElement,
              copy = document.createElement('canvas');
            copy.width = source.width;
            copy.height = source.height;
            const context = copy.getContext('2d')!;
            context.drawImage(source, 0, 0);
            const data = context.getImageData(0, 0, copy.width, copy.height).data;
            resolve(data.filter((_, index) => index % 4 === 3 && data[index]! > 0).length);
          }),
        ),
    );
    expect(nonblank).toBeGreaterThan(1000);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
    await page.screenshot({ path: info.outputPath(`sign-in-${width}.png`), fullPage: true });
    await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
    await page.screenshot({ path: info.outputPath(`sign-in-${width}-dark.png`), fullPage: true });
    const start = Date.now();
    await logo.dblclick();
    await expect(page.getByRole('heading', { name: 'Identity sign-in' })).toBeVisible();
    expect(Date.now() - start).toBeGreaterThan(900);
    expect(signIns).toBe(1);
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.goto('/');
    await expect(logo).toBeEnabled();
    await expect(logo).toHaveAttribute('data-invitation', '');
    await page.screenshot({
      path: info.outputPath(`sign-in-${width}-reduced.png`),
      fullPage: true,
    });
    await logo.press('Enter');
    await expect(page.getByRole('heading', { name: 'Identity sign-in' })).toBeVisible();
    expect(signIns).toBe(2);
    await page.addInitScript(() => {
      Object.defineProperty(window, 'WebGL2RenderingContext', { value: undefined });
    });
    await page.goto('/');
    await expect(logo).toBeEnabled();
    await expect(logo.locator('.landing__character-invitation')).toBeVisible();
    await expect(logo.locator('canvas')).toHaveCount(0);
    await logo.press('Enter');
    await expect(page.getByRole('heading', { name: 'Identity sign-in' })).toBeVisible();
    expect(signIns).toBe(3);
  });

  test(`LOCAL named sandbox overview, Advanced and step motion at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    boundary.agent = {
      ...boundary.agent,
      profile: {
        ...boundary.agent.profile,
        skills: [{ source: 'ornn', id: 'skill-internal', version: '1.2' }],
      },
    };
    await boundary.route(page, baseURL!);
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    const main = page.locator('.sandbox-page');
    await expect(main.getByText('ChronoAI-managed LLM', { exact: true })).toBeVisible();
    await expect(main.getByText('Guarded', { exact: true })).toBeVisible();
    await expect(main.getByText('gpt-proof', { exact: true })).toBeVisible();
    await expect(main.getByRole('link', { name: 'GitHub' })).toHaveAttribute(
      'href',
      'https://nyx.chrono-ai.fun/services/catalog-github',
    );
    await expect(main.getByRole('link', { name: 'TypeScript' })).toHaveAttribute(
      'href',
      'https://ornn.chrono-ai.fun/skills/skill-internal?version=1.2',
    );
    await expect(main.getByText('Workspace limit', { exact: true })).toHaveCount(0);
    await expect(main.getByText('Events', { exact: true })).toHaveCount(0);
    await page.evaluate(() => document.fonts.ready);
    await expect
      .poll(() => main.evaluate((element) => Math.round(element.getBoundingClientRect().width)))
      .toBe(width === 390 ? 334 : 1160);
    await page.screenshot({
      path: info.outputPath(`sandbox-overview-${width}.png`),
      fullPage: true,
    });
    boundary.sandbox = { ...boundary.sandbox, status: 'installing' };
    await page.reload();
    const more = page.getByRole('button', { name: 'More actions', exact: true });
    await more.click();
    await page.getByRole('menuitem', { name: 'Technical details', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Technical details' });
    await expect(dialog.getByText('Workspace limit', { exact: true })).toBeVisible();
    for (const state of ['pending', 'completed']) {
      const logo = dialog.locator(`.landing__character[data-state="${state}"]`).first();
      await expect(logo).toHaveAttribute('data-ready', '');
      const pixels = () =>
        logo.locator('canvas').evaluate(
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
                for (let index = 3; index < data.length; index += 4)
                  if (data[index]! > 0) colored++;
                resolve({ colored, signature: copy.toDataURL() });
              }),
            ),
        );
      const first = await pixels();
      expect(first.colored).toBeGreaterThan(100);
      await expect
        .poll(async () => (await pixels()).signature, { timeout: 5000 })
        .not.toBe(first.signature);
    }
    const geometry = await dialog.evaluate((element) => {
      const box = element.getBoundingClientRect();
      return (
        box.left >= innerWidth * 0.2 - 1 &&
        box.right <= innerWidth * 0.8 + 1 &&
        box.top >= innerHeight * 0.2 - 1 &&
        box.bottom <= innerHeight * 0.8 + 1 &&
        Math.abs((box.left + box.right) / 2 - innerWidth / 2) <= 1 &&
        Math.abs((box.top + box.bottom) / 2 - innerHeight / 2) <= 1 &&
        element.scrollWidth <= element.clientWidth
      );
    });
    expect(geometry).toBe(true);
    await page.screenshot({
      path: info.outputPath(`sandbox-advanced-${width}.png`),
      fullPage: true,
    });
    await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
    await page.screenshot({
      path: info.outputPath(`sandbox-advanced-${width}-dark.png`),
      fullPage: true,
    });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    for (const state of ['pending', 'completed']) {
      const canvas = dialog.locator(`.landing__character[data-state="${state}"] canvas`).first();
      const still = await canvas.screenshot();
      expect(await canvas.screenshot()).toEqual(still);
    }
    await page.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
    await expect(more).toBeFocused();
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}

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
      const create = rect('.sidebar__new'),
        toggle = rect('.sidebar__toggle');
      return {
        fills: tree.height > 600 && account.top - tree.bottom <= 12 && tree.top - head.bottom <= 12,
        separated: create.right <= toggle.left,
        fits: toggle.right <= element.getBoundingClientRect().right,
      };
    });
    expect(geometry).toEqual({ fills: true, separated: true, fits: true });
    await expect(page.locator('.sidebar__brand, .sidebar__build')).toHaveCount(0);
    const brand = page.locator('.service-tabs__brand');
    const badge = page.locator('.service-tabs__build');
    await expect(badge).toBeVisible();
    const brandBox = (await brand.boundingBox())!;
    const badgeBox = (await badge.boundingBox())!;
    expect(badgeBox.x).toBeGreaterThanOrEqual(brandBox.x + brandBox.width);
    expect(badgeBox.y).toBeGreaterThan(brandBox.y);
    await home.click();
    await expect(page).toHaveURL(`${baseURL}/`);
    await expect(page.getByRole('heading', { name: 'ChronoAI Managed Agent' })).toBeVisible();
    await expect(page.locator('.frame')).toBeHidden();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    const character = page.getByRole('button', { name: 'Get Started', exact: true });
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
    await page.screenshot({ path: info.outputPath(`home-${width}.png`), fullPage: true });
    await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
    await page.evaluate(() => document.fonts.ready);
    await expect.poll(async () => (await pixels()).colored).toBeGreaterThan(1000);
    await page.screenshot({ path: info.outputPath(`home-${width}-dark.png`), fullPage: true });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await expect(character).toBeEnabled();
    const still = await canvas.screenshot();
    expect(await canvas.screenshot()).toEqual(still);
    await character.click();
    await expect(page).toHaveURL(`${baseURL}/agent-manager`);
    await expect(page.locator('.frame')).toBeVisible();
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
    await expect(page.getByText(/Current step: home restore/)).toHaveCount(0);
    await page.getByRole('button', { name: 'More actions', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Technical details', exact: true }).click();
    await expect(
      page
        .getByRole('dialog', { name: 'Technical details' })
        .getByText(/Current step: home restore/),
    ).toBeVisible();
    await page.getByRole('button', { name: 'Close', exact: true }).click();
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
      await page.goto('/agent-manager');
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
