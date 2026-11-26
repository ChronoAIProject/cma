import { test, expect, type Request } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';

for (const late of ['accepted', 'store_failed', 'credential_refusal'] as const) {
  test(`LOCAL pending Start survives Cancel, Back and reload after a late ${late}`, async ({
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
    let release!: () => void;
    const pending = new Promise<void>((resolve) => {
      release = resolve;
    });
    let original: Request | undefined;
    const sent: { key?: string; body: string | null }[] = [];
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
        sent.push({ key: request.headers()['idempotency-key'], body: request.postData() });
        if (sent.length === 1) {
          original = request;
          await pending;
          if (late === 'accepted') return reply(accepted, 202);
          return reply(
            {
              title: 'Original response',
              code: late === 'store_failed' ? 'store_failed' : 'sandbox_credential_setup_required',
            },
            late === 'store_failed' ? 503 : 409,
          );
        }
        if (sent.length === 2)
          return reply({ title: 'Receipt unavailable', code: 'store_failed' }, 503);
        current = accepted;
        return reply(accepted, 202);
      }
      if (url.pathname === `/api/v1/launches/${draft.id}` && request.method() === 'GET')
        return reply(current);
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
    await page.goto(`/launches/${draft.id}`);
    await page.getByRole('checkbox').check();
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect.poll(() => sent.length).toBe(1);
    const initial = JSON.parse((await stored())!);
    expect(initial.owner).toBe('local-owner');
    expect(initial.confirmedDigest).toBe('reviewed-commands');
    expect(initial.command.key).toBe(sent[0]?.key);
    expect(JSON.stringify(initial.command.body)).toBe(sent[0]?.body);
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(page).toHaveURL(`${baseURL}/t/public_contract`);
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await page.goBack();
    await expect(page).toHaveURL(`${baseURL}/launches/${draft.id}`);
    await expect(page.getByRole('checkbox')).toBeChecked();
    await expect(page.getByRole('checkbox')).toBeDisabled();
    expect(sent).toHaveLength(1);
    const checkAccount = async () => {
      await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
      await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
      await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
    };
    await checkAccount();
    expect(sent).toHaveLength(1);
    const retry = page.getByRole('button', { name: 'Try again', exact: true });
    await retry.click();
    await expect(retry).toBeEnabled();
    expect(sent).toHaveLength(2);
    expect(sent[1]).toEqual(sent[0]);
    const newer = await stored();
    expect(JSON.parse(newer!).generation).not.toBe(initial.generation);
    const finished = page.waitForEvent('requestfinished', {
      predicate: (request) => request === original,
    });
    release();
    await finished;
    await page.evaluate(
      () =>
        new Promise<void>((done) =>
          requestAnimationFrame(() => requestAnimationFrame(() => done())),
        ),
    );
    expect(await stored()).toBe(newer);
    await expect(page.getByRole('dialog')).toBeVisible();
    await expect(page).toHaveURL(`${baseURL}/launches/${draft.id}`);
    await page.reload();
    await expect(page.getByRole('checkbox')).toBeChecked();
    await expect(page.getByRole('checkbox')).toBeDisabled();
    expect(sent).toHaveLength(2);
    expect(await stored()).toBe(newer);
    await checkAccount();
    expect(sent).toHaveLength(2);
    await retry.click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    expect(sent).toHaveLength(3);
    expect(sent[2]).toEqual(sent[0]);
    expect(await stored()).toBeNull();
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
    await info.attach('local-recovery-custody', {
      body: JSON.stringify({
        mockedHTTP: true,
        actualRouter: true,
        late,
        starts: sent.length,
        distinctKeys: new Set(sent.map((command) => command.key)).size,
        preservedOwnerTrustAndSerializedBody: true,
        providerLoginExecuted: false,
        backendDurabilityProven: false,
      }),
      contentType: 'application/json',
    });
  });
}
