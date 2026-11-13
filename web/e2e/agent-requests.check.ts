import { test, expect } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';
import { PerformanceObserver } from './ui-performance-observer';
import { installIdleTree } from './idle-tree.fixture';

test('LOCAL readiness, idle and visibility request counts', async ({ page, baseURL }, info) => {
  test.setTimeout(150_000);
  const fixture = new LocalAgentV2();
  fixture.boundary.agent = { ...fixture.boundary.agent, status: 'starting' };
  fixture.boundary.sandbox = { ...fixture.boundary.sandbox, status: 'reviving' };
  const observed = new PerformanceObserver(page);
  const candidate = process.env.CMA_REQUEST_BASELINE !== '1';
  const count = (phase: string, path: string) =>
    observed.requests.filter((request) => request.phase === phase && request.path === path).length;
  try {
    await fixture.route(page, baseURL!);
    await observed.delay();
    await page.goto('/agents/agt_contract');
    await page.getByRole('textbox', { name: 'Message', exact: true }).waitFor();
    observed.begin('readiness');
    await page.waitForTimeout(30_000);
    observed.mark('readinessEnd');
    if (candidate)
      expect(count('readiness', '/api/v1/agents/agt_contract')).toBeLessThanOrEqual(18);
    fixture.boundary.agent = { ...fixture.boundary.agent, status: 'ready' };
    fixture.boundary.sandbox = { ...fixture.boundary.sandbox, status: 'active' };
    fixture.append({ type: 'conversation_available' });
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await input.fill('Retained draft');
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled({
      timeout: 15000,
    });
    await page.waitForTimeout(8000);
    observed.begin('idle');
    await page.waitForTimeout(30_000);
    observed.mark('idleEnd');
    // The independent 30-second access revalidation still reads this route.
    if (candidate) expect(count('idle', '/api/v1/agents/agt_contract')).toBeLessThanOrEqual(1);
    await page.evaluate(() => {
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
      Object.defineProperty(document, 'visibilityState', {
        configurable: true,
        get: () => 'hidden',
      });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await page.waitForTimeout(1000);
    observed.begin('hidden');
    await page.waitForTimeout(30_000);
    observed.mark('hiddenEnd');
    expect(count('hidden', '/api/v1/sandboxes/sbx_route')).toBe(0);
    expect(count('hidden', '/api/v1/agents/agt_contract')).toBe(0);
    observed.begin('reopen');
    await page.evaluate(() => {
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
      Object.defineProperty(document, 'visibilityState', {
        configurable: true,
        get: () => 'visible',
      });
      document.dispatchEvent(new Event('visibilitychange'));
      for (let i = 0; i < 20; i++) window.dispatchEvent(new Event('focus'));
    });
    await expect(input).toHaveValue('Retained draft');
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
    await page.waitForTimeout(1000);
    expect(count('reopen', '/api/v1/sandboxes/sbx_route')).toBeLessThanOrEqual(2);
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
    await observed.save(info);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL concurrent slow Sandbox consumers share one request', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalAgentV2();
  const observed = new PerformanceObserver(page);
  let reads = 0;
  try {
    await fixture.route(page, baseURL!);
    await page.route('**/api/v1/sandboxes/sbx_route', async (route) => {
      reads += 1;
      await new Promise((resolve) => setTimeout(resolve, 3000));
      await route.fulfill({ json: fixture.boundary.sandbox });
    });
    await observed.delay();
    await page.goto('/agents/agt_contract');
    await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Retained local draft');
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
    if (process.env.CMA_REQUEST_BASELINE !== '1') expect(reads).toBe(1);
    await observed.save(info);
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

for (const [pages, retained] of [
  [1, true],
  [3, true],
  [10, true],
  [1, false],
] as const) {
  test(`LOCAL identity checks for ${pages} history pages, retained=${retained}`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    const observed = new PerformanceObserver(page);
    try {
      // A retained transport selection isolates the ordinary open path from
      // the one-time eligibility lookup, which must keep its post-IO check.
      if (retained)
        await page.addInitScript(() => {
          sessionStorage.setItem(
            'cma:agent-transport:v1:identity',
            JSON.stringify({
              user: 'local-owner',
              epoch: 'fixture-epoch',
            }),
          );
          sessionStorage.setItem(
            'cma:agent-transport:v1:local-owner:agt_contract',
            JSON.stringify({
              agent: 'agt_contract',
              epoch: 'fixture-epoch',
              transport: 'responses_v2',
              input: null,
              control: null,
            }),
          );
        });
      for (let i = 0; i < pages; i++) fixture.append({ type: 'conversation_available' });
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/agents/agt_contract/events/snapshot**', (route) => {
        const after = Number(
          new URL(route.request().url()).searchParams.get('page_cursor')?.split('.').at(-1) ?? 0,
        );
        return route.fulfill({
          json: {
            object: 'cma.agent.event_page',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(pages),
            items: fixture.items.slice(after, after + 1),
            next_page_cursor:
              after + 1 < pages ? `ags1.${'a'.repeat(64)}.${pages}.${after + 1}` : null,
          },
        });
      });
      await observed.delay();
      await page.goto('/agents/agt_contract');
      const input = page.getByRole('textbox', { name: 'Message', exact: true });
      await input.fill('Retained local draft');
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
      observed.mark('composer');
      await expect.poll(() => fixture.streams.size).toBe(1);
      observed.mark('historyReady');
      const snapshots = observed.requests.filter((request) =>
        request.path.endsWith('/events/snapshot'),
      );
      expect(snapshots).toHaveLength(pages);
      const checks = observed.requests.filter((request) => request.path === '/auth/session');
      const beforeData = checks.filter((request) => request.startMs < snapshots[0]!.startMs).length;
      // App startup has its own identity read and remains outside #1799.
      expect(beforeData - 1).toBe(
        process.env.CMA_IDENTITY_HANDOFF === '1' ? (retained ? 1 : 2) : 3,
      );
      await info.attach('identity-counts', {
        body: JSON.stringify({
          pages,
          retained,
          beforeDataExcludingStartup: beforeData - 1,
          total: checks.length,
        }),
        contentType: 'application/json',
      });
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(observed.errors).toEqual([]);
      await observed.save(info);
    } finally {
      await fixture.close(page);
    }
  });
}

for (const managed of [false, true]) {
  test(`LOCAL twelve-Agent idle tree after management=${managed}`, async ({
    page,
    baseURL,
  }, info) => {
    test.setTimeout(170000);
    const fixture = new LocalAgentV2();
    const observed = new PerformanceObserver(page);
    try {
      await fixture.route(page, baseURL!);
      await installIdleTree(page, fixture);
      await observed.delay();
      await page.goto('/agents/agt_contract');
      await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Retained draft');
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
      if (managed) {
        await page.getByRole('button', { name: 'Filter', exact: true }).click();
        await page.getByRole('button', { name: 'Manage environments', exact: true }).click();
        await page.getByRole('button', { name: 'Back to tasks', exact: true }).click();
      }
      await page.waitForTimeout(3000);
      observed.begin('idleTree');
      await page.waitForTimeout(120000);
      observed.mark('idleTreeEnd');
      const reads = observed.requests.filter((request) => request.phase === 'idleTree');
      if (process.env.CMA_REQUEST_BASELINE !== '1') {
        expect(
          reads.filter((request) => request.path.endsWith('/access-requests')).length,
        ).toBeLessThanOrEqual(4);
        // The open Agent's independent detail projection revalidates private
        // provenance; other Agents isolate the navigation category observer.
        expect(
          reads.filter(
            (request) =>
              request.path.endsWith('/provenance') &&
              request.path !== '/api/v1/agents/agt_contract/provenance',
          ),
        ).toHaveLength(0);
        expect(
          reads.filter((request) => request.path.endsWith('/channel-usage')).length,
        ).toBeLessThanOrEqual(48);
        expect(
          reads.filter((request) => request.path === '/api/v1/sandboxes').length,
        ).toBeLessThanOrEqual(4);
      }
      await page.evaluate(() => {
        Object.defineProperty(document, 'hidden', { configurable: true, get: () => true });
        Object.defineProperty(document, 'visibilityState', {
          configurable: true,
          get: () => 'hidden',
        });
        document.dispatchEvent(new Event('visibilitychange'));
      });
      await page.waitForTimeout(1000);
      observed.begin('hiddenTree');
      await page.waitForTimeout(10000);
      expect(
        observed.requests.filter(
          (request) =>
            request.phase === 'hiddenTree' &&
            /access-requests|channel-usage|provenance|\/sandboxes|\/workspaces/.test(request.path),
        ),
      ).toHaveLength(0);
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(observed.errors).toEqual([]);
      await observed.save(info);
    } finally {
      await fixture.close(page);
    }
  });
}
