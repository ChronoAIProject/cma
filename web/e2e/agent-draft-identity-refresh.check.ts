import { expect, test, type Page } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

const agentPath = '/agents/agt_contract';
const inputPath = '/api/v2/agents/agt_contract/responses';
const message = '  Keep this draft\n🧪  ';
const attachmentName = 'notes.txt';
const attachmentId = `att_${'a'.repeat(64)}`;
const attachmentPath = `/home/attachments/agt_contract/${attachmentId}/${attachmentName}`;

const faults = {
  unauthorized: {
    status: 401,
    json: { authenticated: false },
  },
  'rate limited': {
    status: 429,
    json: { code: 'rate_limited', retryable: true },
    headers: { 'Retry-After': '1' },
  },
  unavailable: {
    status: 503,
    json: { code: 'identity_unavailable', retryable: true },
  },
  malformed: {
    status: 200,
    contentType: 'application/json',
    body: '{',
  },
  timeout: undefined,
};

function composer(page: Page) {
  return page.getByRole('textbox', { name: 'Message', exact: true });
}

function savedDraft(page: Page) {
  return page.evaluate(() =>
    sessionStorage.getItem(
      `cma:agent-draft:${JSON.stringify([location.origin, 'local-owner', 'agt_contract'])}`,
    ),
  );
}

async function openDraft(page: Page) {
  let uploads = 0;
  await page.route('**/api/v1/agents/agt_contract/attachments?*', (route) => {
    expect(route.request().method()).toBe('POST');
    uploads++;
    return route.fulfill({
      status: 201,
      json: {
        id: attachmentId,
        agent_id: 'agt_contract',
        name: attachmentName,
        path: attachmentPath,
        size_bytes: 3,
        media_type: 'text/plain',
        sha256: 'b'.repeat(64),
      },
    });
  });
  await page.goto(agentPath);
  await composer(page).fill(message);
  await page.getByLabel('Choose attachments', { exact: true }).setInputFiles({
    name: attachmentName,
    mimeType: 'text/plain',
    buffer: Buffer.from('abc'),
  });
  await expect(page.getByRole('list', { name: 'Attachments' })).toContainText('Ready');
  expect(uploads).toBe(1);
  return () => expect(uploads).toBe(1);
}

function expectClean(fixture: LocalAgentV2) {
  expect(fixture.errors).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
  expect(fixture.boundary.writes).toEqual([]);
}

for (const [name, response] of Object.entries(faults)) {
  test(`LOCAL TC-104 ${name}: the same Agent draft survives identity refresh and recovers one input`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    let releaseTimeout: (() => void) | undefined;
    try {
      await fixture.route(page, baseURL!);
      const uploadedOnce = await openDraft(page);
      const stored = await savedDraft(page);
      expect(stored).not.toBeNull();
      expect(JSON.parse(stored!)).toMatchObject({
        owner: 'local-owner',
        agentId: 'agt_contract',
        text: message,
        attachments: [{ uploaded: { id: attachmentId, path: attachmentPath } }],
      });
      await expect.poll(() => fixture.streams.size).toBeGreaterThan(0);
      await page.route(
        '**/auth/session',
        async (route) => {
          expect(route.request().method()).toBe('GET');
          if (response) return route.fulfill(response);
          await new Promise<void>((resolve) => {
            releaseTimeout = resolve;
          });
          await route.abort('timedout');
        },
        { times: 1 },
      );
      await page.evaluate(() => window.dispatchEvent(new Event('online')));
      if (name === 'unauthorized') {
        await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
        await expect(composer(page)).toHaveCount(0);
      } else {
        const detail =
          name === 'rate limited'
            ? 'CMA could not verify your sign-in (HTTP 429).'
            : name === 'malformed'
              ? 'identity response was not readable'
              : name === 'timeout'
                ? 'CMA did not respond. Retrying shortly.'
                : 'The identity service is temporarily unavailable. Retrying shortly.';
        await expect(page.getByText(detail, { exact: true })).toBeVisible({ timeout: 12_000 });
      }
      expect(await savedDraft(page)).toBe(stored);
      fixture.endTails();
      const tailReads = () =>
        fixture.reads.filter((path) => path.startsWith('/api/v2/agents/agt_contract/events?'))
          .length;
      const previousTails = tailReads();
      releaseTimeout?.();
      await page.reload();
      await expect(page).toHaveURL(new URL(agentPath, baseURL!).href);
      await expect(composer(page)).toHaveValue(message);
      await expect(page.getByRole('list', { name: 'Attachments' })).toContainText(attachmentName);
      await expect(page.getByRole('list', { name: 'Attachments' })).toContainText('Ready');
      await expect.poll(tailReads).toBeGreaterThan(previousTails);
      expect(await savedDraft(page)).toBe(stored);
      expect(fixture.writes).toEqual([]);

      fixture.loseInput = true;
      fixture.revealInputReceipt = false;
      await page.getByRole('button', { name: 'Send message', exact: true }).click();
      const recover = page.getByRole('button', { name: 'Recover pending request', exact: true });
      await expect(recover).toBeEnabled();
      expect(fixture.writes).toHaveLength(1);
      const write = fixture.writes[0]!;
      expect(write.path).toBe(inputPath);
      expect(write.key).toBeTruthy();
      expect(write.body.input).toContain(message.trim());
      expect(write.body.input).toContain(attachmentPath);

      fixture.revealInputReceipt = true;
      await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
      await recover.click();
      await expect(recover).toHaveCount(0);
      await expect.poll(() => savedDraft(page)).toBeNull();
      expect(fixture.receiptReads.at(-1)).toEqual({ ...write, path: `${inputPath}/receipt` });
      expect(fixture.writes).toEqual([write]);
      uploadedOnce();
      expectClean(fixture);
    } finally {
      releaseTimeout?.();
      await fixture.close(page);
    }
  });
}

for (const exit of ['explicit sign-out', 'session expiry'] as const) {
  test(`LOCAL TC-104 ${exit}: another owner cannot see the previous draft`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    // Use the supported fallback for the decorative landing character so its
    // asynchronous module load does not race the logout navigation.
    await page.addInitScript(() => {
      Object.defineProperty(window, 'WebGL2RenderingContext', { value: undefined });
    });
    try {
      await fixture.route(page, baseURL!);
      const uploadedOnce = await openDraft(page);
      const stored = await savedDraft(page);
      expect(stored).not.toBeNull();
      let signedOut = false;
      await page.route('**/auth/session', (route) =>
        signedOut
          ? route.fulfill({ status: 401, json: { authenticated: false } })
          : route.fallback(),
      );
      if (exit === 'explicit sign-out') {
        await fixture.routeSignOut(page, () => {
          signedOut = true;
        });
        await page.getByRole('button', { name: /Account menu for/ }).click();
        await page.getByRole('menuitem', { name: 'Sign out', exact: true }).click();
        await expect(page).toHaveURL(new URL('/', baseURL!).href);
      } else {
        signedOut = true;
        await page.evaluate(() => window.dispatchEvent(new Event('online')));
      }
      await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
      expect(signedOut).toBe(true);
      await expect(composer(page)).toHaveCount(0);
      expect(await savedDraft(page)).toBe(exit === 'explicit sign-out' ? null : stored);

      fixture.owner = 'second-owner';
      fixture.boundary.sandbox = { ...fixture.boundary.sandbox, owner: fixture.owner };
      signedOut = false;
      if (exit === 'explicit sign-out') await page.goto('/agents/agt_contract');
      else await page.reload();
      await expect(
        page.getByRole('button', { name: 'Account menu for second-owner' }),
      ).toBeVisible();
      await expect(composer(page)).toHaveValue('');
      await expect(page.getByRole('list', { name: 'Attachments' })).toHaveCount(0);
      expect(await savedDraft(page)).toBeNull();
      expect(fixture.writes).toEqual([]);
      uploadedOnce();
      expectClean(fixture);
    } finally {
      await fixture.close(page);
    }
  });
}
