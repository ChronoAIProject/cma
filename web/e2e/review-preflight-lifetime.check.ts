import { test, expect, type Page, type Request, type TestInfo } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';

function gate() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => (resolve = done));
  return { promise, resolve };
}

for (const exit of ['Cancel-Back', 'owner-B-then-A'] as const) {
  test(`LOCAL visible account check cannot enable a replacement Review after ${exit}`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await fixture(page, baseURL!);
    try {
      await flow.prepare();
      const retained = await flow.stored();
      const documentId = await flow.documentId();
      await openAccountCheck(page);
      flow.holdSession();
      await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
      await expect.poll(() => flow.heldCount()).toBe(1);
      await expect(
        page.getByRole('button', { name: 'Checking account', exact: true }),
      ).toBeDisabled();
      expect(flow.heldStatus()).toBe('pending');
      if (exit === 'Cancel-Back') {
        await page.getByRole('button', { name: 'Cancel', exact: true }).click();
        await expect(page).toHaveURL(`${baseURL}/t/public_contract`);
        await expect(page.getByRole('dialog')).toHaveCount(0);
        await expect(page.locator('.public-heading')).toBeVisible();
        await expect.poll(() => flow.heldStatus()).toBe('failed');
        expect(await flow.documentId()).toBe(documentId);
        await page.goBack();
        await expect(page).toHaveURL(/\/launches\//);
      } else {
        flow.setSubject('B');
        await flow.focus();
        await expect(page.locator('.public-progress')).toHaveCount(0);
        await expect(page.getByRole('alert')).toContainText(
          'This request is not available to your account.',
        );
        await expect.poll(() => flow.heldStatus()).toBe('failed');
        flow.setSubject('A');
        await flow.focus();
      }
      await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
      await expect(
        page.getByRole('button', { name: 'Check signed-in account', exact: true }),
      ).toHaveCount(0);
      expect(await flow.documentId()).toBe(documentId);
      expect(flow.starts).toHaveLength(1);
      expect(await flow.stored()).toBe(retained);
      flow.observe('replacement_A_review_visible_before_old_check_release', {
        exit,
        starts: 1,
        browserStatus: flow.heldStatus(),
      });
      await flow.release();
      expect(flow.heldStatus()).toBe('failed');
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
      await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
      expect(await flow.stored()).toBe(retained);
      expect(flow.starts).toHaveLength(1);
      flow.observe('old_visible_check_did_not_enable_new_review', {
        starts: 1,
        browserStatus: flow.heldStatus(),
        fulfilment: flow.heldFulfilment(),
      });
      await page.screenshot({ path: info.outputPath(`aborted-check-${exit}.png`) });
      await checkSameAccount(page);
      expect(flow.starts).toHaveLength(1);
      await page.getByRole('button', { name: 'Try again', exact: true }).click();
      await expect.poll(() => flow.starts.length).toBe(2);
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
      expect(flow.starts[1]).toEqual(flow.starts[0]);
      expect(flow.startSubjects).toEqual(['A', 'A']);
      expect(flow.deniedStarts()).toBe(0);
      expect(recoveryPayload(await flow.stored())).toEqual(recoveryPayload(retained));
      flow.observe('new_explicit_account_check_and_retry_kept_K1', { starts: 2 });
      await flow.finish(info, `visible-check-${exit}`);
    } finally {
      flow.heldResponse.resolve();
    }
  });
}

test('LOCAL Start preflight must not dispatch after global owner change removes its Review', async ({
  page,
  baseURL,
}, info) => {
  const flow = await fixture(page, baseURL!);
  try {
    await flow.prepare();
    await checkSameAccount(page);
    const documentId = await flow.documentId();
    flow.holdSession();
    await page.getByRole('button', { name: 'Try again', exact: true }).click();
    await expect.poll(() => flow.heldCount()).toBe(1);
    const retained = await flow.stored();
    expect(flow.starts).toHaveLength(1);
    flow.setSubject('B');
    await flow.focus();
    await expect(page.locator('.public-progress')).toHaveCount(0);
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('alert')).toContainText(
      'This request is not available to your account.',
    );
    expect(flow.deniedReads()).toBe(1);
    await expect.poll(() => flow.heldStatus()).toBe('failed');
    expect(flow.starts).toHaveLength(1);
    flow.observe('global_B_applied_and_A_review_removed_before_preflight_release', {
      starts: 1,
      browserStatus: flow.heldStatus(),
    });
    await page.screenshot({ path: info.outputPath('B-denied-before-old-preflight.png') });
    await flow.release();
    await page.waitForTimeout(250);
    expect(flow.starts, 'An exited Review must not dispatch a delayed Start').toHaveLength(1);
    expect(flow.heldStatus()).toBe('failed');
    expect(flow.startSubjects).toEqual(['A']);
    expect(flow.deniedStarts()).toBe(0);
    expect(await flow.stored()).toBe(retained);
    await expect(page.locator('.public-progress')).toHaveCount(0);
    await expect(page.getByRole('alert')).toContainText(
      'This request is not available to your account.',
    );
    expect(await flow.documentId()).toBe(documentId);
    flow.observe('late_preflight_cannot_dispatch_after_review_removed', {
      starts: 1,
      deniedStarts: 0,
      recoveryUnchanged: true,
      browserStatus: flow.heldStatus(),
    });
    flow.setSubject('A');
    await flow.focus();
    await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
    expect(await flow.stored()).toBe(retained);
    expect(flow.starts).toHaveLength(1);
    flow.observe('return_A_still_requires_new_check_without_auto_POST', { starts: 1 });
    await page.screenshot({ path: info.outputPath('return-A-after-stale-preflight.png') });
    await checkSameAccount(page);
    expect(flow.starts).toHaveLength(1);
    await page.getByRole('button', { name: 'Try again', exact: true }).click();
    await expect.poll(() => flow.starts.length).toBe(2);
    await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
    expect(flow.starts[1]).toEqual(flow.starts[0]);
    expect(flow.startSubjects).toEqual(['A', 'A']);
    expect(recoveryPayload(await flow.stored())).toEqual(recoveryPayload(retained));
    await flow.finish(info, 'Start-preflight-after-owner-change');
  } finally {
    flow.heldResponse.resolve();
  }
});

function recoveryPayload(raw: string | null) {
  expect(raw).not.toBeNull();
  const { owner, confirmedDigest, command } = JSON.parse(raw!);
  return { owner, confirmedDigest, command };
}

async function openAccountCheck(page: Page) {
  await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
  await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
}

async function checkSameAccount(page: Page) {
  await openAccountCheck(page);
  await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
}

async function fixture(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  let subject: 'A' | 'B' = 'A';
  let holdNext = false;
  let heldCount = 0;
  let heldRequest: Request | undefined;
  let heldStatus: 'none' | 'pending' | 'finished' | 'failed' = 'none';
  let heldFulfilment: 'none' | 'fulfilled' | 'rejected' = 'none';
  let deniedStarts = 0;
  const heldResponse = gate();
  const began = performance.now();
  const sessions: { subject: string; held: boolean; atMs: number; finishedAtMs?: number }[] = [];
  const starts: { key?: string; body: string | null }[] = [];
  const startSubjects: string[] = [];
  const launchReadSubjects: string[] = [];
  const observations: Record<string, unknown>[] = [];
  const unexpected: string[] = [];
  const loginReturns: string[] = [];
  let deniedReads = 0;
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
  const storageKey = `cma:draft-credential-recovery:${draft.id}`;
  page.on('requestfinished', (request) => {
    if (request === heldRequest) heldStatus = 'finished';
  });
  page.on('requestfailed', (request) => {
    if (request === heldRequest) heldStatus = 'failed';
  });
  await page.addInitScript(() => {
    window.open = () => null;
    (window as unknown as { localDocumentId: string }).localDocumentId = crypto.randomUUID();
  });
  await page.context().route('**/auth/login**', async (route) => {
    const url = new URL(route.request().url());
    expect(url.origin).toBe(baseURL);
    loginReturns.push(url.searchParams.get('return_to') ?? '');
    await route.fulfill({
      contentType: 'text/html',
      body: '<!doctype html><title>Local sign-in</title><p>Synthetic account boundary.</p>',
    });
  });
  await page.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== baseURL) return route.fallback();
    const reply = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (url.pathname === '/auth/session') {
      const returnedSubject = subject;
      const observation: (typeof sessions)[number] = {
        subject: returnedSubject,
        held: holdNext,
        atMs: Math.round(performance.now() - began),
      };
      sessions.push(observation);
      if (holdNext) {
        holdNext = false;
        heldCount += 1;
        heldRequest = request;
        heldStatus = 'pending';
        await heldResponse.promise;
      }
      try {
        await reply({
          authenticated: true,
          subject: returnedSubject === 'A' ? 'local-owner' : 'different-owner',
          display_name: `Local owner ${returnedSubject}`,
          visitor: false,
          capabilities: ['use_own_sandboxes', 'launch_triggers'],
        });
        if (observation.held) heldFulfilment = 'fulfilled';
      } catch {
        if (observation.held) heldFulfilment = 'rejected';
      }
      observation.finishedAtMs = Math.round(performance.now() - began);
      return;
    }
    if (url.pathname === `/api/v1/launches/${draft.id}/start` && request.method() === 'POST') {
      starts.push({ key: request.headers()['idempotency-key'], body: request.postData() });
      startSubjects.push(subject);
      if (subject !== 'A') {
        deniedStarts += 1;
        return reply({ code: 'launch_not_found', title: 'Launch not found' }, 404);
      }
      return reply({ code: 'store_failed', title: 'Receipt unavailable' }, 503);
    }
    if (url.pathname === `/api/v1/launches/${draft.id}` && request.method() === 'GET') {
      launchReadSubjects.push(subject);
      if (subject !== 'A') {
        deniedReads += 1;
        return reply({ code: 'launch_not_found', title: 'Launch not found' }, 404);
      }
      return reply(draft);
    }
    if (url.pathname === '/api/v1/triggers/public/public_contract')
      return reply({
        ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
        always_review: true,
      });
    if (url.pathname === `/api/v1/launches/${draft.id}/requirements`)
      return reply(draft.requirements);
    if (url.pathname === `/api/v1/agui/launches/${draft.id}/events`)
      return route.fulfill({ contentType: 'text/event-stream', body: ': local progress\n\n' });
    return route.fallback();
  });
  const frame = () =>
    page.evaluate(
      () =>
        new Promise<void>((done) =>
          requestAnimationFrame(() => requestAnimationFrame(() => done())),
        ),
    );
  const stored = () => page.evaluate((key) => sessionStorage.getItem(key), storageKey);
  const documentId = () =>
    page.evaluate(() => (window as unknown as { localDocumentId: string }).localDocumentId);
  const prepare = async () => {
    await page.goto(`/launches/${draft.id}`);
    await page.getByRole('checkbox').check();
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
    expect(starts).toHaveLength(1);
    expect(await stored()).not.toBeNull();
    await page.reload();
    await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
    expect(starts).toHaveLength(1);
  };
  const release = async () => {
    heldResponse.resolve();
    await expect.poll(() => heldFulfilment).not.toBe('none');
    await expect.poll(() => heldStatus).not.toBe('pending');
    await frame();
  };
  const finish = async (info: TestInfo, scenario: string) => {
    expect(unexpected).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
    expect(boundary.writes).toEqual([]);
    const raw = await stored();
    const packet =
      raw === null
        ? undefined
        : (JSON.parse(raw) as { owner: string; command?: { key: string; body: unknown } });
    const evidence = {
      scenario,
      browserVersion: page.context().browser()?.version(),
      mockedHTTP: true,
      actualAppRouter: true,
      directHookCalls: false,
      directRecoveryWrites: false,
      syntheticFocusOnlineEvents: true,
      sessions,
      starts: starts.length,
      startSubjects,
      distinctKeys: new Set(starts.map((s) => s.key)).size,
      commandSequence: starts.map((s) => starts.findIndex((v) => v.key === s.key) + 1),
      allBodiesEqual: starts.every((s) => s.body === starts[0]?.body),
      launchReadSubjects,
      deniedReads,
      loginReturns,
      heldStatus,
      heldFulfilment,
      deniedStarts,
      finalRecovery: {
        present: packet !== undefined,
        ownerMatchesA: packet?.owner === 'local-owner',
        commandMatchesK1: packet?.command?.key === starts[0]?.key,
        bodyMatches: JSON.stringify(packet?.command?.body) === starts[0]?.body,
      },
      observations,
    };
    console.info('PREFLIGHT_LIFETIME_OBSERVATION', JSON.stringify(evidence));
    await info.attach('preflight-lifetime-evidence', {
      body: JSON.stringify(evidence),
      contentType: 'application/json',
    });
  };
  return {
    sessions,
    starts,
    heldResponse,
    heldStatus: () => heldStatus,
    heldFulfilment: () => heldFulfilment,
    startSubjects,
    deniedStarts: () => deniedStarts,
    heldCount: () => heldCount,
    holdSession: () => {
      holdNext = true;
    },
    setSubject: (next: 'A' | 'B') => {
      subject = next;
    },
    frame,
    focus: () => page.evaluate(() => window.dispatchEvent(new Event('focus'))),
    stored,
    documentId,
    prepare,
    release,
    finish,
    deniedReads: () => deniedReads,
    observe: (phase: string, data: Record<string, unknown>) =>
      observations.push({ phase, ...data }),
  };
}
