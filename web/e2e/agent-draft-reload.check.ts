import { expect, test, type Page, type Request } from '@playwright/test';
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
function settledReads(page: Page) {
  const pending = new Set<Request>();
  let changedAt = Date.now();
  const finished = (request: Request) => {
    if (pending.delete(request)) changedAt = Date.now();
  };
  page.on('request', (request) => {
    if (/^\/(api|auth)\//.test(new URL(request.url()).pathname)) {
      pending.add(request);
      changedAt = Date.now();
    }
  });
  page.on('response', (response) => {
    if (response.headers()['content-type']?.includes('text/event-stream'))
      finished(response.request());
  });
  page.on('requestfinished', finished);
  page.on('requestfailed', finished);
  // A quiet read boundary excludes live SSE bodies, while allowing chained React reads to start.
  return () => expect.poll(() => pending.size === 0 && Date.now() - changedAt >= 500).toBe(true);
}
function clean(fixture: LocalAgentV2) {
  expect(fixture.errors).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
  expect(fixture.boundary.writes).toEqual([]);
}

for (const sameAgent of [false, true]) {
  test(`LOCAL shared access preserves independent drafts and clears prior personal access; same Agent=${sameAgent}`, async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(60_000);
    const fixture = new LocalAgentV2();
    fixture.enabled = false;
    const personal = fixture.boundary.agent;
    const sharedId = sameAgent ? personal.id : 'agt_shared';
    const otherText = '  Another owned Agent draft\n🧪  ';
    const drafts = () =>
      page.evaluate(() =>
        Object.keys(sessionStorage)
          .filter((key) => key.startsWith('cma:agent-draft:'))
          .map(
            (key) => JSON.parse(sessionStorage.getItem(key)!) as { agentId: string; text: string },
          )
          .map(({ agentId, text }) => ({ agentId, text }))
          .sort((a, b) => a.agentId.localeCompare(b.agentId)),
      );
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/api/v1/agents/*/channel-usage', (route) => {
        expect(route.request().method()).toBe('GET');
        return route.fulfill({
          json: {
            agent_id: new URL(route.request().url()).pathname.split('/')[4],
            channel_usage: {
              active: false,
              has_history: false,
              first_used_at: null,
              last_used_at: null,
            },
          },
        });
      });
      await page.route('**/api/v1/agents/provenance?*', (route) => {
        expect(route.request().method()).toBe('GET');
        return route.fulfill({ json: { items: [] } });
      });
      await page.route(`**/api/v2/agents/${sharedId}/events/head`, (route) => {
        expect(route.request().method()).toBe('GET');
        return route.fulfill({
          status: 503,
          json: { code: 'unavailable', detail: 'Optional live head unavailable.' },
        });
      });
      const settled = settledReads(page);
      await page.goto(`/agents/${personal.id}`);
      await field(page).fill(original);
      await settled();
      fixture.boundary.agent = { ...personal, id: 'agt_other' };
      await page.goto('/agents/agt_other');
      await field(page).fill(otherText);
      await settled();
      const before = await drafts();
      expect(before).toEqual([
        { agentId: personal.id, text: original },
        { agentId: 'agt_other', text: otherText },
      ]);
      fixture.boundary.agent = { ...personal, id: sharedId };
      await page.route(`**/api/v1/agents/${sharedId}`, (route) => {
        if (route.request().method() !== 'GET') return route.fallback();
        return route.fulfill({
          json: {
            ...personal,
            access: 'shared',
            id: sharedId,
            title: 'Legitimate shared Agent',
          },
        });
      });
      await page.route(`**/api/v1/resource-access/agent/${sharedId}`, (route) =>
        route.fulfill({
          json: {
            capabilities: ['view_details', 'view_history'],
            owner: { kind: 'person', is_current_person: false },
            policy_revision: 1,
            revalidate_at: null,
            shared_by: {
              subject: 'another-owner',
              display_name: 'Another Owner',
              avatar_url: null,
            },
          },
        }),
      );
      await page.goto(`/agents/${sharedId}`);
      await expect(page.getByRole('heading', { name: 'Legitimate shared Agent' })).toBeVisible();
      await expect(page.getByText('View-only access', { exact: false })).toBeVisible();
      await expect(field(page)).not.toBeVisible();
      expect(await drafts()).toEqual(sameAgent ? [] : before);
      await settled();
      await page.reload();
      await expect(page.getByRole('heading', { name: 'Legitimate shared Agent' })).toBeVisible();
      expect(await drafts()).toEqual(sameAgent ? [] : before);
      await settled();
      await page.unroute(`**/api/v1/agents/${sharedId}`);
      await page.unroute(`**/api/v1/resource-access/agent/${sharedId}`);
      for (const [id, text] of [
        [personal.id, original],
        ['agt_other', otherText],
      ]) {
        fixture.boundary.agent = { ...personal, id: id! };
        await page.goto(`/agents/${id}`);
        await expect(field(page)).toHaveValue(sameAgent ? '' : text!);
        await settled();
        await page.reload();
        await expect(field(page)).toHaveValue(sameAgent ? '' : text!);
        await settled();
      }
      expect(fixture.writes).toEqual([]);
      clean(fixture);
    } finally {
      await fixture.close(page);
    }
  });
}

test('LOCAL exact draft survives actual reload, list/reopen and reconnect without command writes', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  try {
    await fixture.route(page, baseURL!);
    const settled = settledReads(page);
    await page.goto('/agents/agt_contract');
    await field(page).fill(original);
    await settled();
    await page.reload();
    await expect(field(page)).toHaveValue(original);
    await settled();
    expect(fixture.boundary.errors, 'before opening the list').toEqual([]);
    await page.goto('/agent-manager');
    await settled();
    await page.goto('/agents/agt_contract');
    await expect(field(page)).toHaveValue(original);
    await settled();
    const tails = () =>
      fixture.reads.filter((path) => path.startsWith('/api/v2/agents/agt_contract/events?')).length;
    expect(fixture.boundary.errors, 'before reconnecting').toEqual([]);
    const beforeReconnect = tails();
    fixture.endTails();
    await expect.poll(tails).toBeGreaterThan(beforeReconnect);
    await expect(send(page)).toBeEnabled();
    await expect(field(page)).toHaveValue(original);
    await settled();
    await page.reload();
    await expect(field(page)).toHaveValue(original);
    expect(fixture.writes).toEqual([]);
    await settled();
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
    const settled = settledReads(page);
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
    await settled();
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
    await settled();
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
    await settled();
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
      const settled = settledReads(page);
      let uncertain = true;
      fixture.revealInputReceipt = false;
      await page.route('**/api/v2/agents/agt_contract/responses', (route) => {
        if (!uncertain || route.request().method() !== 'POST') return route.fallback();
        uncertain = false;
        const write = {
          path: '/api/v2/agents/agt_contract/responses',
          key: route.request().headers()['idempotency-key'],
          body: route.request().postDataJSON() as Record<string, unknown>,
        };
        fixture.writes.push(write);
        fixture.admitInput(write);
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
      await settled();
      await page.reload();
      await expect(field(page)).toHaveValue(original);
      await expect(recover).toBeEnabled();
      // The loading notice's exit frame can move the button between pointer-down and pointer-up.
      await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
      expect(fixture.writes).toHaveLength(1);
      if (edit) await field(page).fill('A newer unsent draft');
      fixture.revealInputReceipt = true;
      await recover.click();
      await expect(recover).not.toBeVisible();
      await expect(field(page)).toHaveValue(edit ? 'A newer unsent draft' : '');
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.receiptReads.at(-1)).toEqual({
        ...fixture.writes[0],
        path: `${fixture.writes[0]!.path}/receipt`,
      });
      await settled();
      await page.reload();
      await expect(field(page)).toHaveValue(edit ? 'A newer unsent draft' : '');
      expect(fixture.writes).toHaveLength(1);
      await settled();
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
    const settled = settledReads(page);
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
    await settled();
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL incompatible draft recovery is explicit and never discards current edits on retry', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  try {
    await fixture.route(page, baseURL!);
    const settled = settledReads(page);
    await page.goto('/agents/agt_contract');
    await expect(field(page)).toBeVisible();
    await page.evaluate(() => {
      sessionStorage.removeItem('cma:agent-draft-owner');
      sessionStorage.setItem(
        `cma:agent-draft:${JSON.stringify([location.origin, 'local-owner', 'agt_contract'])}`,
        '{"version":0}',
      );
    });
    await settled();
    await page.reload();
    await expect(
      page.getByRole('alert').filter({ hasText: 'could not be recovered' }),
    ).toBeVisible();
    await field(page).fill(original);
    await page.getByRole('button', { name: 'Retry draft storage' }).click();
    await expect(field(page)).toHaveValue(original);
    await expect(send(page)).toBeDisabled();
    await page.getByRole('button', { name: 'Discard saved draft' }).click();
    await expect(send(page)).toBeEnabled();
    await settled();
    await page.reload();
    await expect(field(page)).toHaveValue(original);
    expect(fixture.writes).toEqual([]);
    await settled();
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});
