import { test, expect } from '@playwright/test';
import { workspaceAgentButton } from './lifecycle-creation';
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

test('LOCAL J6 selects the admitted Workspace among duplicate names and overlapping paths', async ({
  page,
}) => {
  await page.setContent(`
    <!doctype html><title>Local Workspace selection</title>
    <section class="sandbox-page__workspace">
      <h3>Shared profile</h3><p><code>/workspaces/lifecycle-existing</code></p>
      <button data-workspace="wks_existing">New agent</button>
    </section>
    <section class="sandbox-page__workspace">
      <h3>Shared profile</h3><p><code>/workspaces/lifecycle</code></p>
      <button data-workspace="wks_lifecycle">New agent</button>
    </section>
    <section class="sandbox-page__workspace">
      <h3>Shared profile</h3><p><code>/workspaces/lifecycle/nested</code></p>
      <button data-workspace="wks_nested">New agent</button>
    </section>
    <output aria-label="Selected Workspace"></output>
  `);
  await page.evaluate(() => {
    for (const button of document.querySelectorAll<HTMLButtonElement>('button')) {
      button.addEventListener('click', () => {
        document.querySelector('output')!.textContent = button.dataset.workspace!;
      });
    }
  });
  const workspace = { id: 'wks_lifecycle', path: '/workspaces/lifecycle' };
  const button = workspaceAgentButton(page, workspace);
  await expect(page.getByRole('button', { name: 'New agent', exact: true })).toHaveCount(3);
  await expect(button).toHaveCount(1);
  await button.click();
  await expect(page.getByLabel('Selected Workspace')).toHaveText(workspace.id);
  await page.evaluate(() => {
    document.body.append(
      document
        .querySelector('button[data-workspace="wks_existing"]')!
        .closest('.sandbox-page__workspace')!,
    );
    document.querySelector('output')!.textContent = '';
  });
  await button.click();
  await expect(page.getByLabel('Selected Workspace')).toHaveText(workspace.id);
  await expect(
    workspaceAgentButton(page, { id: 'wks_missing', path: '/workspaces/missing' }),
  ).toHaveCount(0);
});
