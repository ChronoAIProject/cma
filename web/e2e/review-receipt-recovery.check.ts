import { test, expect } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';

test('LOCAL expired-lease response with Draft retains the Start until its original receipt arrives', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  let launch: Launch = {
    ...recorded<Launch>('launches.json', 'launch-ready-create'),
    review: {
      needs: [
        { type: 'owner_requires_review' },
        { type: 'trust_confirmation', commands_digest: 'reviewed-commands' },
      ],
      can_start_now: false,
    },
  };
  const commands: { key?: string; body: string | null }[] = [];
  const statesRead: string[] = [];
  let originalCompleted = false;
  await page
    .context()
    .route('**/auth/login**', (route) =>
      route.fulfill({ contentType: 'text/html', body: '<title>Local sign-in fixture</title>' }),
    );
  await page.addInitScript(() => {
    window.open = () => null;
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
    if (url.pathname === `/api/v1/launches/${launch.id}/start` && request.method() === 'POST') {
      commands.push({ key: request.headers()['idempotency-key'], body: request.postData() });
      if (commands.length === 1) return route.abort('connectionfailed');
      if (originalCompleted) return reply(launch, 202);
      return reply(
        {
          title: 'Command receipt unavailable',
          code: commands.length === 2 ? 'idempotency_in_flight' : 'idempotency_recovery_required',
        },
        409,
      );
    }
    if (url.pathname === `/api/v1/launches/${launch.id}`) {
      statesRead.push(launch.state);
      return reply(launch);
    }
    if (url.pathname === '/api/v1/triggers/public/public_contract')
      return reply({
        ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
        always_review: true,
      });
    if (url.pathname === `/api/v1/launches/${launch.id}/requirements`)
      return reply(launch.requirements);
    if (url.pathname === `/api/v1/agui/launches/${launch.id}/events`)
      return route.fulfill({ contentType: 'text/event-stream', body: ': local progress\n\n' });
    return route.fallback();
  });
  await page.clock.install();
  await page.goto(`/launches/${launch.id}`);
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  const retry = page.getByRole('button', { name: 'Try again', exact: true });
  await expect(retry).toBeEnabled();
  expect(commands).toHaveLength(1);
  const storageKey = `cma:draft-credential-recovery:${launch.id}`;
  const retained = await page.evaluate((key) => sessionStorage.getItem(key), storageKey);
  const packet = JSON.parse(retained!);
  expect(packet.owner).toBe('local-owner');
  expect(packet.confirmedDigest).toBe('reviewed-commands');
  expect(packet.command.key).toBeTruthy();
  expect(packet.command.key).toBe(commands[0]?.key);
  expect(JSON.stringify(packet.command.body)).toBe(commands[0]?.body);
  const retainedCommand = async () => {
    const raw = await page.evaluate((key) => sessionStorage.getItem(key), storageKey);
    expect(raw).not.toBeNull();
    expect(JSON.parse(raw!)).toEqual({ ...packet, generation: expect.any(String) });
    return raw;
  };
  await retry.click();
  await expect(retry).toBeEnabled();
  expect(commands).toHaveLength(2);
  const inFlightAttempt = await retainedCommand();
  expect(inFlightAttempt).not.toBe(retained);
  await page.clock.fastForward(120_001);
  expect(commands).toHaveLength(2);
  expect(await retainedCommand()).toBe(inFlightAttempt);
  const readsBeforeRecovery = statesRead.length;
  await retry.click();
  await expect(retry).toBeEnabled();
  await expect.poll(() => statesRead.length).toBeGreaterThan(readsBeforeRecovery);
  expect(statesRead.at(-1)).toBe('draft');
  expect(commands).toHaveLength(3);
  await expect(page.getByRole('dialog')).toBeVisible();
  await expect(page.getByRole('checkbox')).toBeChecked();
  await expect(page.getByRole('checkbox')).toBeDisabled();
  const recoveryAttempt = await retainedCommand();
  expect(recoveryAttempt).not.toBe(inFlightAttempt);
  await page.screenshot({ path: info.outputPath('recovery-required-still-draft.png') });
  await page.reload();
  await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
  await expect(retry).toBeDisabled();
  await expect(page.getByRole('checkbox')).toBeChecked();
  await expect(page.getByRole('checkbox')).toBeDisabled();
  expect(commands).toHaveLength(3);
  expect(await retainedCommand()).toBe(recoveryAttempt);
  await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
  await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
  await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
  await expect(retry).toBeEnabled();
  expect(commands).toHaveLength(3);
  expect(await retainedCommand()).toBe(recoveryAttempt);
  originalCompleted = true;
  launch = recorded<Launch>('launches.json', 'launch-ready-start');
  await retry.click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.locator('.public-progress')).toBeVisible();
  expect(commands).toHaveLength(4);
  for (const command of commands) expect(command).toEqual(commands[0]);
  expect(await page.evaluate((key) => sessionStorage.getItem(key), storageKey)).toBeNull();
  await page.reload();
  await expect(page.locator('.public-progress')).toBeVisible();
  expect(commands).toHaveLength(4);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
  await info.attach('local-live-lease-recovery-boundary', {
    body: JSON.stringify({
      mocked: true,
      boundary: 'built_browser_public_http',
      mongoExecuted: false,
      actualLeaseExpiryExecuted: false,
      simulatedBrowserElapsedMs: 120_001,
      recoveryRead: 'draft',
      distinctCommands: 1,
      writes: commands.length,
      recoveredOriginalReceipt: true,
    }),
    contentType: 'application/json',
  });
});

test('LOCAL Launch receipt recovery retains the original command through auth and storage failures', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  let launch: Launch = {
    ...recorded<Launch>('launches.json', 'launch-ready-create'),
    review: {
      needs: [
        { type: 'owner_requires_review' },
        { type: 'trust_confirmation', commands_digest: 'reviewed-commands' },
      ],
      can_start_now: false,
    },
  };
  const commands: { key?: string; body: string | null }[] = [];
  const loginReturns: string[] = [];
  let subject = 'local-owner';
  await page.context().route('**/auth/login**', async (route) => {
    const url = new URL(route.request().url());
    expect(url.origin).toBe(baseURL);
    loginReturns.push(url.searchParams.get('return_to') ?? '');
    await route.fulfill({
      contentType: 'text/html',
      body: '<!doctype html><title>Local sign-in boundary</title><p>Local identity fixture only.</p>',
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
        subject,
        display_name: 'Local owner',
        visitor: false,
        capabilities: ['use_own_sandboxes', 'launch_triggers'],
      });
    if (url.pathname === `/api/v1/launches/${launch.id}/start` && request.method() === 'POST') {
      commands.push({ key: request.headers()['idempotency-key'], body: request.postData() });
      if (commands.length === 1)
        return reply({ title: 'Sign in again', code: 'sandbox_credential_setup_required' }, 409);
      if (commands.length === 2) return route.abort('connectionfailed');
      if (commands.length === 3)
        return reply({ title: 'Sign-in required', code: 'not_authenticated' }, 401);
      launch = recorded<Launch>('launches.json', 'launch-ready-start');
      return reply(launch, 202);
    }
    if (url.pathname === '/api/v1/triggers/public/public_contract')
      return reply({
        ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
        always_review: true,
      });
    if (url.pathname === `/api/v1/launches/${launch.id}`) return reply(launch);
    if (url.pathname === `/api/v1/launches/${launch.id}/requirements`)
      return reply(launch.requirements);
    if (url.pathname === `/api/v1/agui/launches/${launch.id}/events`)
      return route.fulfill({ contentType: 'text/event-stream', body: ': local progress\n\n' });
    return route.fallback();
  });
  await page.addInitScript(() => {
    window.open = () => null;
    const getItem = Storage.prototype.getItem;
    Storage.prototype.getItem = function (key: string) {
      if (
        key.startsWith('cma:draft-credential-recovery:') &&
        getItem.call(this, 'local-recovery-read-blocked') === 'true'
      )
        throw new DOMException('Synthetic storage denial', 'SecurityError');
      return getItem.call(this, key);
    };
  });
  await page.goto(`/launches/${launch.id}`);
  await page.getByRole('checkbox').check();
  expect(commands).toHaveLength(0);
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
  expect(commands).toHaveLength(1);
  const checkAccount = async () => {
    await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
    await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
    await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
  };
  await checkAccount();
  await expect(page.getByRole('button', { name: 'Continue', exact: true })).toBeEnabled();
  expect(commands).toHaveLength(1);
  await page.getByRole('button', { name: 'Continue', exact: true }).click();
  const retry = page.getByRole('button', { name: 'Try again', exact: true });
  await expect(retry).toBeEnabled();
  expect(commands).toHaveLength(2);
  expect(commands[1]?.key).not.toBe(commands[0]?.key);
  expect(commands[1]?.body).toBe(commands[0]?.body);
  await retry.click();
  await expect(retry).toBeEnabled();
  expect(commands).toHaveLength(3);
  expect(commands[2]).toEqual(commands[1]);
  await expect(page.getByRole('checkbox')).toBeDisabled();

  const storageKey = `cma:draft-credential-recovery:${launch.id}`;
  const original = await page.evaluate((key) => sessionStorage.getItem(key), storageKey);
  expect(JSON.parse(original!).command.key).toBe(commands[1]?.key);
  await page.evaluate(() => sessionStorage.setItem('local-recovery-read-blocked', 'true'));
  subject = 'different-owner';
  await page.reload();
  await expect(
    page.getByText('Your saved request could not be read. Allow session storage and try again.'),
  ).toBeVisible();
  await expect(page.getByRole('checkbox')).toBeDisabled();
  await retry.click();
  expect(commands).toHaveLength(3);
  await page.screenshot({ path: info.outputPath('recovery-storage-blocked.png') });
  await page.evaluate(() => sessionStorage.removeItem('local-recovery-read-blocked'));
  await retry.click();
  await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
  expect(commands).toHaveLength(3);
  await expect(page.getByRole('checkbox')).toBeChecked();
  await checkAccount();
  await expect(
    page.getByText('Sign in with the same CMA account that made this request.'),
  ).toBeVisible();
  await expect(retry).toBeDisabled();
  expect(await page.evaluate((key) => sessionStorage.getItem(key), storageKey)).toBe(original);
  subject = 'local-owner';
  await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
  await expect(retry).toBeEnabled();
  expect(commands).toHaveLength(3);
  await retry.click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  expect(commands).toHaveLength(4);
  expect(commands[3]).toEqual(commands[1]);
  expect(await page.evaluate((key) => sessionStorage.getItem(key), storageKey)).toBeNull();
  await page.reload();
  await expect(page.locator('.public-progress')).toBeVisible();
  expect(commands).toHaveLength(4);
  expect(loginReturns).toEqual(['/t/public_contract', '/t/public_contract']);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
  await info.attach('local-receipt-recovery-boundary', {
    body: JSON.stringify({
      mocked: true,
      boundary: 'built_browser_public_http',
      providerLoginExecuted: false,
      receiptDurabilityProven: false,
      distinctCommands: 2,
      writes: commands.length,
      recoveredOriginalSerializedBody: true,
      retainedThroughAuthAndStorageFailure: true,
    }),
    contentType: 'application/json',
  });
});

for (const receipt of ['unwritten', 'saved'] as const) {
  test(`LOCAL ${receipt} Launch receipt preserves command identity after a completion error`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    let launch: Launch = {
      ...recorded<Launch>('launches.json', 'launch-ready-create'),
      review: {
        needs: [
          { type: 'owner_requires_review' },
          { type: 'trust_confirmation', commands_digest: 'reviewed-commands' },
        ],
        can_start_now: false,
      },
    };
    const commands: { key?: string; body: string | null }[] = [];
    const statesRead: string[] = [];
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
      if (url.pathname === `/api/v1/launches/${launch.id}/start` && request.method() === 'POST') {
        commands.push({ key: request.headers()['idempotency-key'], body: request.postData() });
        if (commands.length === 1) {
          launch = recorded<Launch>('launches.json', 'launch-ready-start');
          return reply({ title: 'Receipt completion unavailable', code: 'store_failed' }, 503);
        }
        if (receipt === 'saved') return reply(launch, 202);
        return reply(
          {
            title: 'Command receipt unavailable',
            code: commands.length === 2 ? 'idempotency_in_flight' : 'idempotency_recovery_required',
          },
          409,
        );
      }
      if (url.pathname === `/api/v1/launches/${launch.id}`) {
        statesRead.push(launch.state);
        return reply(launch);
      }
      if (url.pathname === '/api/v1/triggers/public/public_contract')
        return reply({
          ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
          always_review: true,
        });
      if (url.pathname === `/api/v1/launches/${launch.id}/requirements`)
        return reply(launch.requirements);
      if (url.pathname === `/api/v1/agui/launches/${launch.id}/events`)
        return route.fulfill({ contentType: 'text/event-stream', body: ': local progress\n\n' });
      return route.fallback();
    });
    await page.goto(`/launches/${launch.id}`);
    await page.getByRole('checkbox').check();
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    const retry = page.getByRole('button', { name: 'Try again', exact: true });
    await expect(retry).toBeEnabled();
    expect(commands).toHaveLength(1);
    const storageKey = `cma:draft-credential-recovery:${launch.id}`;
    const retained = await page.evaluate((key) => sessionStorage.getItem(key), storageKey);
    const packet = JSON.parse(retained!);
    expect(packet.owner).toBe('local-owner');
    expect(packet.command.key).toBe(commands[0]?.key);
    expect(JSON.stringify(packet.command.body)).toBe(commands[0]?.body);
    await expect(page.getByRole('checkbox')).toBeChecked();
    await expect(page.getByRole('checkbox')).toBeDisabled();
    await retry.click();
    if (receipt === 'unwritten') {
      await expect(retry).toBeEnabled();
      expect(commands).toHaveLength(2);
      const retried = await page.evaluate((key) => sessionStorage.getItem(key), storageKey);
      expect(JSON.parse(retried!)).toEqual({ ...packet, generation: expect.any(String) });
      expect(JSON.stringify(JSON.parse(retried!).command.body)).toBe(commands[0]?.body);
      expect(retried).not.toBe(retained);
      await retry.click();
    }
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.locator('.public-progress')).toBeVisible();
    expect(commands).toHaveLength(receipt === 'unwritten' ? 3 : 2);
    expect(commands.every((command) => command.key === commands[0]?.key)).toBe(true);
    expect(commands.every((command) => command.body === commands[0]?.body)).toBe(true);
    await expect.poll(() => statesRead.includes('resolving')).toBe(true);
    const expectedStored = await page.evaluate((key) => sessionStorage.getItem(key), storageKey);
    if (receipt === 'unwritten') {
      expect(JSON.parse(expectedStored!)).toEqual({ ...packet, generation: expect.any(String) });
      expect(JSON.stringify(JSON.parse(expectedStored!).command.body)).toBe(commands[0]?.body);
    } else expect(expectedStored).toBeNull();
    await page.reload();
    await expect(page.locator('.public-progress')).toBeVisible();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    expect(commands).toHaveLength(receipt === 'unwritten' ? 3 : 2);
    expect(await page.evaluate((key) => sessionStorage.getItem(key), storageKey)).toBe(
      expectedStored,
    );
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
    await info.attach('local-receipt-completion-boundary', {
      body: JSON.stringify({
        mocked: true,
        boundary: 'built_browser_public_http',
        receipt,
        mongoExecuted: false,
        processRestartExecuted: false,
        firstResponse: '503 store_failed',
        recoveredReceipt: receipt === 'saved',
        unresolvedIdentityRetained: receipt === 'unwritten',
        distinctCommands: 1,
        writes: commands.length,
      }),
      contentType: 'application/json',
    });
  });
}
