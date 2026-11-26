import { expect, type Page, type Request, type TestInfo } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { recorded } from './agent-startup-fixtures';
export const originalPath = '/t/pub_session/plc_a';
export const ownerA = 'local-owner-a';
export const ownerB = 'local-owner-b';
interface Packet {
  id: string;
  key: string;
  path: string;
  owner: string;
  expiresAt: number;
  submitted: boolean;
  credentialRecovery?: string;
  wireBody?: string;
  body: unknown;
}
export async function identityFixture(page: Page, baseURL: string, credentialFlow = true) {
  const accepted: Launch = {
    ...recorded<Launch>('launches.json', 'launch-ready-create'),
    id: 'lch_session_boundary_receipt',
    public_id: 'pub_session',
    state: 'resolving',
    revision: 2,
    event_head: 2,
    review: { needs: [], can_start_now: false },
  };
  let unverified: number | undefined;
  let admissions = 0;
  const posts: {
    expected?: string;
    path: string;
    key?: string;
    body: string | null;
    cookieOwner: string;
    status: number;
    finished: boolean;
  }[] = [];
  const sessions: {
    page: string;
    capturedOwner: string;
    status: number;
    held: boolean;
    released: boolean;
  }[] = [];
  const indices = new Map<Request, number>();
  const loginReturns: string[] = [];
  const observations: Record<string, unknown>[] = [];
  const unexpected: string[] = [];
  const errors: string[] = [];
  let unavailable = false;
  let holdNext = false;
  let release: (() => void) | undefined;
  const context = page.context();
  const switchOwner = (value: string) =>
    context.addCookies([
      { name: 'cma_fixture_owner', value, url: baseURL, httpOnly: true, sameSite: 'Lax' },
    ]);
  await switchOwner(ownerA);
  page.on('pageerror', (error) => errors.push(error.message));
  context.on('page', (opened) => opened.on('pageerror', (error) => errors.push(error.message)));
  context.on('requestfinished', (request) => {
    const index = indices.get(request);
    if (index !== undefined) posts[index]!.finished = true;
  });
  await context.addInitScript(() => {
    (window as unknown as { localDocumentId: string }).localDocumentId = crypto.randomUUID();
  });
  await context.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const reply = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
    if (url.origin !== baseURL) {
      unexpected.push('external_request');
      return route.abort();
    }
    if (url.pathname === '/auth/session') {
      const cookie = await request.headerValue('cookie');
      const capturedOwner =
        /(?:^|;\s*)cma_fixture_owner=([^;]+)/.exec(cookie ?? '')?.[1] ?? 'anonymous';
      const held = holdNext && request.frame().page() === page;
      if (held) holdNext = false;
      const row = {
        page: request.frame().page() === page ? 'original' : 'popup',
        capturedOwner,
        status: unavailable ? 503 : capturedOwner === 'anonymous' ? 401 : 200,
        held,
        released: !held,
      };
      sessions.push(row);
      if (held) {
        await new Promise<void>((resolve) => {
          release = resolve;
        });
        row.released = true;
      }
      return reply(
        row.status !== 200
          ? {
              authenticated: false,
              ...(row.status === 503
                ? { code: 'store_failed', title: 'Synthetic session read unavailable' }
                : {}),
            }
          : {
              authenticated: true,
              subject: capturedOwner,
              display_name: capturedOwner === ownerA ? 'Synthetic A' : 'Synthetic B',
              visitor: false,
              capabilities: ['use_own_sandboxes', 'launch_triggers'],
            },
        row.status,
      );
    }
    if (url.pathname === '/api/v1/ui/image-manager-retirement' && request.method() === 'GET')
      return reply(false);
    if (url.pathname === '/api/v1/profile-authoring/services' && request.method() === 'GET')
      return reply([{ id: 'api-github', catalog_id: 'catalog-github', name: 'GitHub' }]);
    if (url.pathname === '/auth/login') {
      const target = url.searchParams.get('return_to');
      if (target !== '/t/pub_session' && target !== originalPath) {
        unexpected.push('unexpected_login_return');
        return route.abort();
      }
      loginReturns.push(target);
      return route.fulfill({ status: 302, headers: { location: target }, body: '' });
    }
    if (/^\/api\/v[12]\/triggers\/public\/pub_session\/launches$/.test(url.pathname)) {
      expect(request.method()).toBe('POST');
      const body = request.postData();
      const key = request.headers()['idempotency-key'];
      const cookie = await request.headerValue('cookie');
      const cookieOwner =
        /(?:^|;\s*)cma_fixture_owner=([^;]+)/.exec(cookie ?? '')?.[1] ?? 'anonymous';
      const expected = request.headers()['x-cma-expected-subject'];
      const verified = `v1.${Buffer.from(cookieOwner).toString('base64url')}`;
      const bound = url.pathname.includes('/v2/');
      const mismatch = bound && expected !== verified;
      const previous = posts[1];
      const forced = unverified;
      unverified = undefined;
      const status = mismatch
        ? 409
        : (forced ??
          (!credentialFlow
            ? 201
            : admissions === 0
              ? 409
              : admissions === 1
                ? 503
                : previous !== undefined && previous.key === key && previous.body === body
                  ? 201
                  : 422));
      if (!mismatch) admissions++;
      indices.set(request, posts.length);
      posts.push({ key, body, cookieOwner, expected, path: url.pathname, status, finished: false });
      return route.fulfill({
        status,
        headers:
          bound && !mismatch && forced === undefined ? { 'X-CMA-Verified-Subject': verified } : {},
        contentType: 'application/json',
        body: JSON.stringify(
          status === 201
            ? accepted
            : {
                code: mismatch
                  ? 'command_subject_mismatch'
                  : status === 409
                    ? 'sandbox_credential_setup_required'
                    : status === 429
                      ? 'resource_quota'
                      : status === 404
                        ? 'not_found'
                        : status === 503
                          ? 'store_failed'
                          : 'idempotency_key_reused',
                title: 'Synthetic command disposition',
                status,
              },
        ),
      });
    }
    if (url.pathname === '/api/v1/triggers/public/pub_session')
      return reply({
        ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
        title: 'Session boundary entry',
        always_review: true,
      });
    if (url.pathname === `/api/v1/launches/${accepted.id}`) return reply(accepted);
    if (url.pathname === `/api/v1/launches/${accepted.id}/requirements`)
      return reply(accepted.requirements);
    if (url.pathname === `/api/v1/agui/launches/${accepted.id}/events`)
      return route.fulfill({ contentType: 'text/event-stream', body: ': synthetic progress\n\n' });
    if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/auth/')) {
      unexpected.push(`${request.method()} ${url.pathname}`);
      return reply({ code: 'unexpected_local_request' }, 500);
    }
    if (request.method() === 'GET') return route.continue();
    unexpected.push(`${request.method()} ${url.pathname}`);
    return route.abort();
  });
  const stored = () => page.evaluate(() => sessionStorage.getItem('cma:public-intent'));
  const packet = async () => JSON.parse((await stored()) ?? 'null') as Packet | null;
  const openLogin = async () => {
    const before = posts.length;
    const opened = context.waitForEvent('page');
    await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
    const popup = await opened;
    await expect(
      popup.getByRole('heading', { name: 'Session boundary entry', exact: true }),
    ).toBeVisible();
    expect(await popup.evaluate(() => window.opener === null)).toBe(true);
    expect(posts).toHaveLength(before);
    return popup;
  };
  return {
    posts,
    accepted,
    admissions: () => admissions,
    unverified: (status: number) => {
      unverified = status;
    },
    packet,
    stored,
    switchOwner,
    openLogin,
    unavailable: (value: boolean) => {
      unavailable = value;
    },
    holdNextOriginalSession: () => {
      expect(holdNext).toBe(false);
      expect(release).toBeUndefined();
      holdNext = true;
    },
    waitHeld: () => expect.poll(() => release !== undefined).toBe(true),
    releaseHeld: () => {
      const resume = release;
      release = undefined;
      if (resume === undefined) throw new Error('No session response held');
      resume();
    },
    documentId: () =>
      page.evaluate(() => (window as unknown as { localDocumentId: string }).localDocumentId),
    waitCount: async (count: number) => {
      await expect.poll(() => posts.length).toBe(count);
      await expect.poll(() => posts.every((post) => post.finished)).toBe(true);
    },
    authorizeOriginal: async () => {
      const before = posts.length;
      const popup = await openLogin();
      await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeEnabled();
      expect(posts).toHaveLength(before);
      await popup.close();
    },
    observe: (phase: string, data: Record<string, unknown>) =>
      observations.push({ phase, ...data }),
    finish: async (info: TestInfo) => {
      release?.();
      const data = {
        credentialFlow,
        browserVersion: context.browser()?.version(),
        actualCompiledAppRouterSDKAndStorage: true,
        syntheticHTTPLoginAndSessionCookie: true,
        syntheticCookieName: 'cma_fixture_owner',
        cookieValuesAreNotCredentials: true,
        noClockManipulation: true,
        directStorageWrites: false,
        backendExecuted: false,
        posts,
        sessions,
        loginReturns,
        observations,
        finalPath: new URL(page.url()).pathname,
        finalPacket: await packet(),
        unexpected,
        pageErrors: errors,
      };
      await info.attach('entry-credential-session-evidence', {
        body: JSON.stringify(data),
        contentType: 'application/json',
      });
      expect(unexpected).toEqual([]);
      expect(errors).toEqual([]);
    },
  };
}
