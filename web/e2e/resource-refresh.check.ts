import { test, expect } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';
import { desktopBoundary } from './desktop-rfb.fixture';

for (const mode of ['unrelated', 'pending-history', 'own-sandbox'] as const) {
  test(`LOCAL resource refresh preserves Agent view: ${mode}`, async ({ page, baseURL }, info) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
    for (let i = 1; i <= 3; i++)
      fixture.append({ type: 'message', role: 'assistant', content: `Retained history page ${i}` });
    let release!: () => void;
    const pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    const requests: string[] = [];
    const pages: number[] = [];
    const commands: { id: string; kind: string; key?: string }[] = [];
    let status: 'active' | 'pausing' | 'paused' | 'reviving' = 'active';
    const own = mode === 'own-sandbox';
    const id = own ? 'sbx_route' : 'sbx_other';
    const name = own ? fixture.boundary.sandbox.name : 'Other environment';
    const target = () => ({
      ...fixture.boundary.sandbox,
      id,
      name,
      status,
      workspaces: own ? fixture.boundary.sandbox.workspaces : [],
      lifecycle: {
        pause: status === 'active' ? { allowed: true } : { allowed: false, reason: 'not_active' },
        revive: status === 'paused' ? { allowed: true } : { allowed: false, reason: 'not_paused' },
      },
    });
    try {
      await fixture.route(page, baseURL!);
      const desktop = await desktopBoundary(page);
      page.on('request', (request) => requests.push(new URL(request.url()).pathname));
      await page.route(/\/api\/v1\/sandboxes(?:\?|$)/, (route) =>
        route.fulfill({ json: { items: own ? [target()] : [fixture.boundary.sandbox, target()] } }),
      );
      await page.route(`**/api/v1/sandboxes/${id}`, (route) => route.fulfill({ json: target() }));
      await page.route(`**/api/v1/sandboxes/${id}/*`, async (route) => {
        const kind = new URL(route.request().url()).pathname.split('/').at(-1)!;
        if (route.request().method() !== 'POST' || !['pause', 'revive'].includes(kind))
          return route.fallback();
        commands.push({ id, kind, key: route.request().headers()['idempotency-key'] });
        status = kind === 'pause' ? 'pausing' : 'reviving';
        await route.fulfill({ status: 202, json: target() });
      });
      if (own)
        await page.route('**/api/v1/agents/agt_contract', (route) =>
          route.fulfill({
            json: {
              ...fixture.boundary.agent,
              sandbox: { ...fixture.boundary.agent.sandbox, status },
            },
          }),
        );
      await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(3),
            active_turn: null,
          },
        }),
      );
      await page.route('**/api/v2/agents/agt_contract/events/snapshot*', async (route) => {
        const token = new URL(route.request().url()).searchParams.get('page_cursor');
        const index = token === null ? 0 : Number(token.split('.').at(-1));
        pages.push(index);
        if (index === 1 && mode === 'pending-history') await pending;
        await route.fulfill({
          json: {
            object: 'cma.agent.event_page',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(3),
            items: [fixture.items[index]],
            next_page_cursor: index < 2 ? `ags1.${'a'.repeat(64)}.3.${index + 1}` : null,
          },
        });
      });
      await page.goto('/agents/agt_contract');
      const panel = page.getByRole('region', { name: 'Machine desktop', exact: true });
      await expect(panel.getByRole('status')).toContainText('Desktop is idle');
      await expect.poll(() => fixture.streams.size).toBe(1);
      await expect.poll(() => pages.length).toBe(mode === 'pending-history' ? 2 : 3);
      const message = page.getByRole('textbox', { name: 'Message', exact: true });
      await message.fill('Retain my resource-operation draft');
      const composer = await message.elementHandle();
      const panelNode = await panel.elementHandle();
      if (mode === 'unrelated')
        await panel.getByRole('button', { name: 'Take control', exact: true }).click();
      await page.getByRole('button', { name: 'Filter', exact: true }).click();
      await page.getByRole('button', { name: 'Manage environments', exact: true }).click();
      const before = requests.length;
      await page.getByRole('button', { name: `Options for ${name}`, exact: true }).click();
      await page.getByRole('menuitem', { name: 'Pause', exact: true }).click();
      await expect(page.getByText('Pause request accepted.', { exact: true })).toBeVisible();
      status = 'paused';
      await expect(page.getByText('Sandbox paused.', { exact: true })).toBeVisible();
      const afterPause = requests.slice(before);
      const historyReads = (paths: string[]) =>
        paths.filter((path) =>
          /agt_contract\/(transcript|usage|events\/(head|snapshot))$/.test(path),
        );
      expect(historyReads(afterPause)).toEqual([]);
      if (!own) {
        expect(
          afterPause.filter(
            (path) => path === '/auth/session' || path === '/api/v1/agents/agt_contract',
          ),
        ).toEqual([]);
        expect(desktop.connections).toHaveLength(1);
        if (mode === 'unrelated')
          await expect(
            panel.getByRole('button', { name: 'Return to Agent', exact: true }),
          ).toBeVisible();
      } else {
        await expect(panel.getByRole('status')).toContainText('Waiting for your machine');
        expect(afterPause.filter((path) => path === '/api/v1/agents/agt_contract')).toHaveLength(2);
        await page.getByRole('button', { name: `Options for ${name}`, exact: true }).click();
        await page.getByRole('menuitem', { name: 'Resume', exact: true }).click();
        await expect(page.getByText('Resume request accepted.', { exact: true })).toBeVisible();
        status = 'active';
        await expect(page.getByText('Sandbox resumed.', { exact: true })).toBeVisible();
        await expect(panel.getByRole('status')).toContainText('Desktop is idle');
        expect(historyReads(requests.slice(before))).toEqual([]);
      }
      if (mode === 'pending-history') {
        expect(pages).toEqual([0, 1]);
        release();
        await expect(page.getByText('Retained history page 3', { exact: true })).toBeVisible();
      }
      expect(pages).toEqual([0, 1, 2]);
      expect(await message.evaluate((node, previous) => node === previous, composer)).toBe(true);
      expect(await panel.evaluate((node, previous) => node === previous, panelNode)).toBe(true);
      await expect(message).toHaveValue('Retain my resource-operation draft');
      expect(commands.map(({ kind }) => kind)).toEqual(own ? ['pause', 'revive'] : ['pause']);
      expect(commands.every((command) => !!command.key)).toBe(true);
      expect(fixture.errors).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      await info.attach('request-counts.json', {
        body: JSON.stringify({
          mode,
          pages,
          afterPause,
          commands: commands.map(({ id, kind }) => ({ id, kind })),
          connections: desktop.connections.length,
        }),
        contentType: 'application/json',
      });
    } finally {
      release();
      await fixture.close(page);
    }
  });
}
