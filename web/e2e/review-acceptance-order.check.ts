import { test, expect, type Page, type Request } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';

function gate() {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => (release = resolve));
  return { promise, release };
}

async function checkAccount(page: Page) {
  await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
  await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
  await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
}

for (const order of ['accepted-first', 'error-first'] as const) {
  test(`LOCAL same-key acceptance survives an older error arriving ${order}`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    const draft: Launch = {
      ...recorded<Launch>('launches.json', 'launch-ready-create'),
      review: {
        needs: [
          { type: 'owner_requires_review' },
          { type: 'trust_confirmation', commands_digest: 'reviewed-commands' },
        ],
        can_start_now: false,
      },
    };
    const accepted = recorded<Launch>('launches.json', 'launch-ready-start');
    let current = draft;
    const older = gate();
    const newer = gate();
    const starts: { key?: string; body: string | null }[] = [];
    const requests: Request[] = [];
    const reads: string[] = [];
    const storageKey = `cma:draft-credential-recovery:${draft.id}`;
    const stored = () => page.evaluate((key) => sessionStorage.getItem(key), storageKey);
    await page.addInitScript(() => {
      window.open = () => null;
    });
    await page.context().route('**/auth/login**', async (route) => {
      expect(new URL(route.request().url()).origin).toBe(baseURL);
      await route.fulfill({
        contentType: 'text/html',
        body: '<!doctype html><title>Local sign-in</title><p>Synthetic identity boundary.</p>',
      });
    });
    await page.route('**/*', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (url.origin !== baseURL) return route.fallback();
      const reply = (body: unknown, status = 200) =>
        route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
      if (url.pathname === '/auth/session')
        return reply({
          authenticated: true,
          subject: 'local-owner',
          display_name: 'Local owner',
          visitor: false,
          capabilities: ['use_own_sandboxes', 'launch_triggers'],
        });
      if (url.pathname === `/api/v1/launches/${draft.id}/start` && request.method() === 'POST') {
        starts.push({ key: request.headers()['idempotency-key'], body: request.postData() });
        requests.push(request);
        if (starts.length === 1)
          return reply({ title: 'Initial receipt unavailable', code: 'store_failed' }, 503);
        if (starts.length === 2) {
          await older.promise;
          return reply({ title: 'Older receipt unavailable', code: 'store_failed' }, 503);
        }
        if (starts.length === 3) {
          await newer.promise;
          current = accepted;
          return reply(accepted, 202);
        }
        return reply({ title: 'Unexpected extra Start', code: 'unexpected_local_write' }, 500);
      }
      if (url.pathname === `/api/v1/launches/${draft.id}` && request.method() === 'GET') {
        reads.push(current.state);
        return reply(current);
      }
      if (url.pathname === '/api/v1/triggers/public/public_contract')
        return reply({
          ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
          always_review: true,
        });
      if (url.pathname === `/api/v1/launches/${draft.id}/requirements`)
        return reply(current.requirements);
      if (url.pathname === `/api/v1/agui/launches/${draft.id}/events`)
        return route.fulfill({ contentType: 'text/event-stream', body: ': local progress\n\n' });
      return route.fallback();
    });
    const release = async (response: ReturnType<typeof gate>, request: Request) => {
      const finished = page.waitForEvent('requestfinished', {
        predicate: (value) => value === request,
      });
      response.release();
      await finished;
      await page.evaluate(
        () =>
          new Promise<void>((done) =>
            requestAnimationFrame(() => requestAnimationFrame(() => done())),
          ),
      );
    };
    try {
      await page.goto(`/launches/${draft.id}`);
      await page.getByRole('checkbox').check();
      await page.getByRole('button', { name: 'Continue', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
      expect(starts).toHaveLength(1);
      const command = {
        owner: 'local-owner',
        confirmedDigest: 'reviewed-commands',
        command: { key: starts[0]!.key, body: JSON.parse(starts[0]!.body!) },
      };
      expect(JSON.parse((await stored())!)).toMatchObject(command);
      await page.reload();
      await checkAccount(page);
      expect(starts).toHaveLength(1);
      await page.getByRole('button', { name: 'Try again', exact: true }).click();
      await expect.poll(() => starts.length).toBe(2);
      await page.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(page).toHaveURL(`${baseURL}/t/public_contract`);
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await page.goBack();
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
      await expect(page.getByRole('checkbox')).toBeChecked();
      await expect(page.getByRole('checkbox')).toBeDisabled();
      expect(JSON.parse((await stored())!)).toMatchObject(command);
      await checkAccount(page);
      expect(starts).toHaveLength(2);
      await page.getByRole('button', { name: 'Try again', exact: true }).click();
      await expect.poll(() => starts.length).toBe(3);
      expect(starts[1]).toEqual(starts[0]);
      expect(starts[2]).toEqual(starts[0]);
      const retained = await stored();
      expect(JSON.parse(retained!)).toMatchObject(command);
      if (order === 'error-first') {
        await release(older, requests[1]!);
        expect(await stored()).toBe(retained);
      }
      await release(newer, requests[2]!);
      await expect(page.locator('.public-progress > .badge')).toHaveText('In progress');
      await expect(page.getByRole('dialog')).toHaveCount(0);
      expect(await stored()).toBeNull();
      if (order === 'accepted-first') await release(older, requests[1]!);
      expect(await stored()).toBeNull();
      await expect(page.locator('.public-progress > .badge')).toHaveText('In progress');
      await page.reload();
      await expect(page.locator('.public-progress > .badge')).toHaveText('In progress');
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toHaveCount(0);
      expect(starts).toHaveLength(3);
      expect(reads.at(-1)).toBe('resolving');
      expect(await stored()).toBeNull();
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
      await info.attach('same-key-acceptance', {
        body: JSON.stringify({
          order,
          starts,
          reads,
          mockedHTTP: true,
          directRecoveryWrites: false,
          backendDurabilityProven: false,
        }),
        contentType: 'application/json',
      });
    } finally {
      older.release();
      newer.release();
    }
  });
}
