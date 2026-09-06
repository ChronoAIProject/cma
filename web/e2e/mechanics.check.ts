import { test, expect } from '@playwright/test';
import { assertPageSafe, readOnlyGuard } from './security';

// This suite tests the harness against intercepted local HTML, never staging journeys.
test('LOCAL smoke guard allows reads and refuses browser POST and DELETE', async ({ page }) => {
  const blocked: string[] = [];
  const served: string[] = [];
  await page.context().route('**/*', async (route) => {
    served.push(route.request().method());
    await route.fulfill({
      contentType: 'text/html',
      body: '<!doctype html><title>Local guard check</title><p>Public state</p>',
    });
  });
  await readOnlyGuard(page.context(), blocked);
  await page.goto('https://example.test/t/local-fixture');
  for (const method of ['POST', 'DELETE']) {
    const rejected = await page.evaluate(async (method) => {
      try {
        await fetch('/api/v1/sandboxes', { method });
        return false;
      } catch {
        return true;
      }
    }, method);
    expect(rejected).toBe(true);
  }
  expect(blocked).toEqual(['POST', 'DELETE']);
  expect(served.every((method) => method === 'GET')).toBe(true);
});

test('LOCAL credential checks reject storage, DOM and identity-provider pages before capture', async ({
  page,
}) => {
  await page.context().route('**/*', (route) =>
    route.fulfill({
      contentType: 'text/html',
      body: '<!doctype html><title>Local capture check</title><p>Public state</p>',
    }),
  );
  await page.goto('https://example.test/agents/local-agent');
  await expect(
    assertPageSafe(page, 'https://example.test', ['redacted-probe']),
  ).resolves.toBeUndefined();
  await page.evaluate(() => sessionStorage.setItem('unsafe', 'redacted-probe'));
  await expect(assertPageSafe(page, 'https://example.test', ['redacted-probe'])).rejects.toThrow(
    'Credential material',
  );
  await page.evaluate(() => {
    sessionStorage.clear();
    document.body.innerHTML = '<input type="password">';
  });
  await expect(assertPageSafe(page, 'https://example.test', [])).rejects.toThrow(
    'Credential-entry',
  );
  await page.goto('https://identity.example.test/register');
  await expect(assertPageSafe(page, 'https://example.test', [])).rejects.toThrow('canonical CMA');
});
