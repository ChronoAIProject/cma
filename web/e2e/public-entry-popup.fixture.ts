import { verifiedEntryHeaders } from './public-entry-binding.fixture';
import { expect, type Page, type Request, type TestInfo } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { recorded } from './agent-startup-fixtures';

export const placementPath = '/t/pub_placement/plc_a';

export async function popupFixture(page: Page, baseURL: string) {
  const accepted: Launch = {
    ...recorded<Launch>('launches.json', 'launch-ready-create'),
    id: 'lch_placement_receipt',
    public_id: 'pub_placement',
    state: 'resolving',
    revision: 2,
    event_head: 2,
    review: { needs: [], can_start_now: false },
  };
  const posts: {
    page: 'original' | 'popup';
    key?: string;
    body: string | null;
    status: number;
    finished: boolean;
  }[] = [];
  const indices = new Map<Request, number>();
  const loginReturns: string[] = [];
  const unexpected: string[] = [];
  const errors: string[] = [];
  let subject = 'local-owner';
  const context = page.context();
  page.on('pageerror', (error) => errors.push(error.message));
  context.on('page', (opened) => opened.on('pageerror', (error) => errors.push(error.message)));
  context.on('requestfinished', (request) => {
    const index = indices.get(request);
    if (index !== undefined) posts[index]!.finished = true;
  });
  await context.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const proof = verifiedEntryHeaders(request, subject);
    const reply = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        headers: proof,
        contentType: 'application/json',
        body: JSON.stringify(body),
      });
    if (url.origin !== baseURL) {
      unexpected.push('external_request');
      return route.abort();
    }
    if (url.pathname === '/auth/session')
      return reply({
        authenticated: true,
        subject,
        display_name: 'Local owner',
        visitor: false,
        capabilities: ['use_own_sandboxes', 'launch_triggers'],
      });
    if (url.pathname === '/api/v1/ui/image-manager-retirement' && request.method() === 'GET')
      return reply(false);
    if (url.pathname === '/api/v1/profile-authoring/services' && request.method() === 'GET')
      return reply([{ id: 'api-github', catalog_id: 'catalog-github', name: 'GitHub' }]);
    if (url.pathname === '/auth/login') {
      const target = url.searchParams.get('return_to');
      if (target !== '/t/pub_placement' && target !== placementPath) {
        unexpected.push('unexpected_login_return');
        return route.abort();
      }
      loginReturns.push(target);
      return route.fulfill({ status: 302, headers: { location: target }, body: '' });
    }
    if (url.pathname === '/api/v2/triggers/public/pub_placement/launches') {
      expect(request.method()).toBe('POST');
      const body = request.postData();
      const key = request.headers()['idempotency-key'];
      const original = posts[0];
      const status = original === undefined ? 409 : original.body === body ? 201 : 422;
      indices.set(request, posts.length);
      posts.push({
        page: request.frame().page() === page ? 'original' : 'popup',
        key,
        body,
        status,
        finished: false,
      });
      return reply(
        status === 201
          ? accepted
          : status === 409
            ? {
                code: 'sandbox_credential_setup_required',
                title: 'sandbox credential setup required',
                detail: 'Sign in to CMA with the same account.',
                status: 409,
              }
            : { code: 'idempotency_key_reused', title: 'Request bytes changed', status: 422 },
        status,
      );
    }
    if (url.pathname === '/api/v1/triggers/public/pub_placement')
      return reply({
        ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
        title: 'Placement entry',
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
  const stored = (target: Page) =>
    target.evaluate(() => sessionStorage.getItem('cma:public-intent'));
  return {
    posts,
    loginReturns,
    stored,
    setSubject: (value: string) => {
      subject = value;
    },
    waitCount: async (count: number) => {
      await expect.poll(() => posts.length).toBe(count);
      await expect.poll(() => posts.every((post) => post.finished)).toBe(true);
    },
    finish: async (info: TestInfo) => {
      await info.attach('entry-popup-boundary', {
        body: JSON.stringify({
          browserVersion: context.browser()?.version(),
          syntheticHTTPAndIdentity: true,
          syntheticLogin302: true,
          backendExecuted: false,
          posts,
          loginReturns,
          unexpected,
          pageErrors: errors,
        }),
        contentType: 'application/json',
      });
      expect(unexpected).toEqual([]);
      expect(errors).toEqual([]);
    },
  };
}
