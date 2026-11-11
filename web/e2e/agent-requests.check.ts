import { test, expect } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';
import { PerformanceObserver } from './ui-performance-observer';

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
