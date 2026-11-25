import { expect, test, type Request } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';

for (const order of ['queued refresh', 'preflight before global A', 'local A before global B']) {
  test(`LOCAL ${order} preserves explicit Launch recovery`, async ({ page, baseURL }, info) => {
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
    let current = draft;
    let subject = 'local-owner';
    let holdNext = false;
    let heldRequest: Request | undefined;
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    const sessions: string[] = [];
    const readers: string[] = [];
    const starts: { subject: string; key?: string; body: string | null }[] = [];
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
      if (url.pathname === '/auth/session') {
        const snapshot = subject;
        sessions.push(snapshot);
        if (holdNext) {
          holdNext = false;
          heldRequest = request;
          await held;
        }
        return reply({
          authenticated: true,
          subject: snapshot,
          display_name: snapshot,
          visitor: false,
          capabilities: ['use_own_sandboxes', 'launch_triggers'],
        });
      }
      if (url.pathname === `/api/v1/launches/${draft.id}/start` && request.method() === 'POST') {
        starts.push({
          subject,
          key: request.headers()['idempotency-key'],
          body: request.postData(),
        });
        if (subject !== 'local-owner') return reply({ title: 'Not found', code: 'not_found' }, 404);
        if (starts.length === 1)
          return reply({ title: 'Receipt unavailable', code: 'store_failed' }, 503);
        current = recorded<Launch>('launches.json', 'launch-ready-start');
        return reply(current, 202);
      }
      if (url.pathname === `/api/v1/launches/${draft.id}` && request.method() === 'GET') {
        readers.push(subject);
        return subject === 'local-owner'
          ? reply(current)
          : reply({ title: 'Not found', code: 'not_found' }, 404);
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
    const checkAccount = async () => {
      await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
      await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
      await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
    };
    try {
      await page.goto(`/launches/${draft.id}`);
      await page.getByRole('checkbox').check();
      await page.getByRole('button', { name: 'Continue', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
      expect(starts).toHaveLength(1);
      await page.reload();
      await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
      if (order !== 'local A before global B') await checkAccount();
      const retained = await stored();
      expect(JSON.parse(retained!)).toMatchObject({
        owner: 'local-owner',
        confirmedDigest: 'reviewed-commands',
        command: { key: starts[0]!.key, body: JSON.parse(starts[0]!.body!) },
      });
      if (order === 'local A before global B') subject = 'different-owner';
      holdNext = true;
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      await expect.poll(() => heldRequest !== undefined).toBe(true);
      const before = sessions.length;
      let afterPreflight = retained;
      if (order === 'queued refresh') {
        subject = 'different-owner';
        await page.evaluate(() => {
          window.dispatchEvent(new Event('focus'));
          window.dispatchEvent(new Event('online'));
          document.dispatchEvent(new Event('visibilitychange'));
        });
        expect(sessions).toHaveLength(before);
      } else if (order === 'preflight before global A') {
        subject = 'different-owner';
        await page.getByRole('button', { name: 'Try again', exact: true }).click();
        await expect(
          page.getByText('Sign in with the same CMA account that made this request.'),
        ).toBeVisible();
        await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
        expect(sessions).toHaveLength(before + 1);
        expect(sessions.at(-1)).toBe('different-owner');
        afterPreflight = await stored();
        expect(JSON.parse(afterPreflight!)).toMatchObject({
          owner: 'local-owner',
          confirmedDigest: 'reviewed-commands',
          command: { key: starts[0]!.key, body: JSON.parse(starts[0]!.body!) },
        });
      } else {
        subject = 'local-owner';
        await checkAccount();
        expect(sessions).toHaveLength(before + 1);
        expect(sessions.at(-1)).toBe('local-owner');
        expect(await stored()).toBe(retained);
      }
      expect(starts).toHaveLength(1);
      const finished = page.waitForEvent('requestfinished', {
        predicate: (request) => request === heldRequest,
      });
      release();
      await finished;
      if (order === 'preflight before global A') {
        await expect(
          page.getByText('Sign in with the same CMA account that made this request.'),
        ).toBeVisible();
        await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
        expect(sessions).toHaveLength(before + 1);
        expect(await stored()).toBe(afterPreflight);
        await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      }
      if (order === 'local A before global B') {
        await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
        await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
        expect(sessions).toHaveLength(before + 1);
      } else {
        const expectedQueries = before + (order === 'queued refresh' ? 1 : 2);
        await expect.poll(() => sessions.length).toBe(expectedQueries);
        expect(sessions.at(-1)).toBe('different-owner');
        await expect.poll(() => readers.at(-1)).toBe('different-owner');
        await expect(page.getByRole('dialog')).toHaveCount(0);
      }
      expect(starts).toHaveLength(1);
      expect(await stored()).toBe(afterPreflight);

      subject = 'local-owner';
      const beforeReturn = readers.length;
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      await expect.poll(() => readers.length).toBeGreaterThan(beforeReturn);
      expect(readers.at(-1)).toBe('local-owner');
      await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
      await expect(page.getByRole('checkbox')).toBeChecked();
      await expect(page.getByRole('checkbox')).toBeDisabled();
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
      expect(starts).toHaveLength(1);
      expect(await stored()).toBe(afterPreflight);
      await checkAccount();
      expect(starts).toHaveLength(1);
      await page.getByRole('button', { name: 'Try again', exact: true }).click();
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await expect(page.locator('.public-progress > .badge')).toHaveText('In progress');
      expect(starts).toHaveLength(2);
      expect(starts[1]).toEqual(starts[0]);
      expect(await stored()).toBeNull();
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
      await info.attach('identity-refresh-recovery', {
        body: JSON.stringify({
          order,
          sessions,
          readers,
          starts,
          mockedHTTP: true,
          providerLoginExecuted: false,
          backendDurabilityProven: false,
        }),
        contentType: 'application/json',
      });
    } finally {
      release();
    }
  });
}
