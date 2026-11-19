import { expect, test, type Page } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

const original = '  Unsent 草稿 🧪\n\tPreserve whitespace.  \n';
const field = (page: Page) => page.getByRole('textbox', { name: 'Message', exact: true });
const send = (page: Page) => page.getByRole('button', { name: 'Send message', exact: true });
const saved = (page: Page) =>
  page.evaluate(() =>
    sessionStorage.getItem(
      `cma:agent-draft:${JSON.stringify([location.origin, 'local-owner', 'agt_contract'])}`,
    ),
  );
function clean(fixture: LocalAgentV2) {
  expect(fixture.errors).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
  expect(fixture.boundary.writes).toEqual([]);
}

test('LOCAL exact draft survives actual reload, list/reopen and reconnect without command writes', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  try {
    await fixture.route(page, baseURL!);
    let identityReads = 0;
    page.on('request', (request) => {
      if (request.url().endsWith('/auth/session')) identityReads++;
    });
    const settled = (request: import('@playwright/test').Request) => {
      if (request.url().endsWith('/auth/session')) identityReads--;
    };
    page.on('requestfinished', settled);
    page.on('requestfailed', settled);
    await page.goto('/agents/agt_contract');
    await field(page).fill(original);
    await expect.poll(() => identityReads).toBe(0);
    await page.reload();
    await expect(field(page)).toHaveValue(original);
    await expect.poll(() => identityReads).toBe(0);
    await page.goto('/agent-manager');
    await expect.poll(() => identityReads).toBe(0);
    await page.goto('/agents/agt_contract');
    await expect(field(page)).toHaveValue(original);
    fixture.endTails();
    await Promise.all([
      page.waitForResponse((response) => response.url().endsWith('/auth/session')),
      page.evaluate(() => window.dispatchEvent(new Event('online'))),
    ]);
    await expect.poll(() => identityReads).toBe(0);
    await expect(send(page)).toBeEnabled();
    await expect(field(page)).toHaveValue(original);
    await page.reload();
    await expect(field(page)).toHaveValue(original);
    expect(fixture.writes).toEqual([]);
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL uploaded references survive reload while unfinished files require removal or reselection', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  let uploads = 0;
  const id = `att_${'a'.repeat(64)}`;
  const path = `/home/attachments/agt_contract/${id}/notes.txt`;
  try {
    await fixture.route(page, baseURL!);
    await page.route('**/api/v1/agents/agt_contract/attachments?*', (route) => {
      uploads++;
      return uploads === 1
        ? route.fulfill({
            status: 201,
            json: {
              id,
              agent_id: 'agt_contract',
              name: 'notes.txt',
              path,
              size_bytes: 3,
              media_type: 'text/plain',
              sha256: 'b'.repeat(64),
            },
          })
        : route.fulfill({ status: 503, json: { message: 'Temporarily unavailable' } });
    });
    await page.goto('/agents/agt_contract');
    await field(page).fill(original);
    const picker = page.getByLabel('Choose attachments', { exact: true });
    await expect(picker).toBeEnabled();
    await picker.setInputFiles({
      name: 'notes.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('abc'),
    });
    await expect(page.getByRole('list', { name: 'Attachments' })).toContainText('Ready');
    const before = JSON.parse((await saved(page))!);
    await page.reload();
    await expect(field(page)).toHaveValue(original);
    await expect(page.getByText('notes.txt', { exact: true })).toBeVisible();
    await expect(page.getByText('Ready', { exact: true })).toBeVisible();
    expect(JSON.parse((await saved(page))!).attachments).toEqual(before.attachments);
    expect(uploads).toBe(1);
    await expect(picker).toBeEnabled();
    await picker.setInputFiles({
      name: 'unfinished.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('new'),
    });
    await expect.poll(() => uploads).toBe(2);
    await page.reload();
    await expect(field(page)).toHaveValue(original);
    await expect(page.getByText('Reselect this file or remove it before sending.')).toBeVisible();
    await expect(send(page)).toBeDisabled();
    expect(uploads).toBe(2);
    expect(fixture.writes).toEqual([]);
    await page.getByRole('button', { name: 'Remove unfinished.txt', exact: true }).click();
    await send(page).click();
    await expect.poll(() => fixture.writes.length).toBe(1);
    expect(fixture.writes[0]!.body.input).toContain(path);
    await expect.poll(() => saved(page)).toBeNull();
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});

for (const edit of [false, true]) {
  test(`LOCAL uncertain submission survives reload and recovers the same command; newer draft=${edit}`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    try {
      await fixture.route(page, baseURL!);
      let uncertain = true;
      await page.route('**/api/v2/agents/agt_contract/responses', (route) => {
        if (!uncertain || route.request().method() !== 'POST') return route.fallback();
        uncertain = false;
        fixture.writes.push({
          path: '/api/v2/agents/agt_contract/responses',
          key: route.request().headers()['idempotency-key'],
          body: route.request().postDataJSON() as Record<string, unknown>,
        });
        return route.fulfill({ status: 502, json: { message: 'Response unavailable' } });
      });
      await page.goto('/agents/agt_contract');
      await field(page).fill(original);
      await send(page).click();
      const recover = page.getByRole('button', { name: 'Recover pending request', exact: true });
      await expect(recover).toBeEnabled();
      expect(fixture.writes).toHaveLength(1);
      const key = fixture.writes[0]!.key;
      expect(JSON.parse((await saved(page))!).commandKey).toBe(key);
      await page.reload();
      await expect(field(page)).toHaveValue(original);
      await expect(recover).toBeEnabled();
      expect(fixture.writes).toHaveLength(1);
      if (edit) await field(page).fill('A newer unsent draft');
      await recover.click();
      await expect(recover).not.toBeVisible();
      await expect(field(page)).toHaveValue(edit ? 'A newer unsent draft' : '');
      expect(fixture.writes).toHaveLength(2);
      expect(fixture.writes[1]!.key).toBe(key);
      expect(fixture.writes[1]!.body).toEqual(fixture.writes[0]!.body);
      await page.reload();
      await expect(field(page)).toHaveValue(edit ? 'A newer unsent draft' : '');
      expect(fixture.writes).toHaveLength(2);
      clean(fixture);
    } finally {
      await fixture.close(page);
    }
  });
}

test('LOCAL storage failure keeps current text and offers recovery actions', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  try {
    await fixture.route(page, baseURL!);
    await page.addInitScript(() => {
      const set = Storage.prototype.setItem;
      Storage.prototype.setItem = function (key, value) {
        if (key.startsWith('cma:agent-draft:'))
          throw new DOMException('Full', 'QuotaExceededError');
        return set.call(this, key, value);
      };
    });
    await page.goto('/agents/agt_contract');
    await field(page).fill(original);
    await expect(field(page)).toHaveValue(original);
    await expect(
      page.getByRole('alert').filter({ hasText: 'Your draft was not saved to this browser.' }),
    ).toBeVisible();
    await expect(page.getByRole('button', { name: 'Retry draft storage' })).toBeVisible();
    expect(fixture.writes).toEqual([]);
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});
