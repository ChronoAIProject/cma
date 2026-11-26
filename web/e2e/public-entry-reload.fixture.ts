import { verifiedEntryHeaders } from './public-entry-binding.fixture';
import { expect, type Page, type Request } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';
import type { EntryPacket } from './public-entry-lifecycle.fixture';

export type ReloadScenario =
  'receipt' | 'pending' | 'recovery-required' | 'expired-fence' | 'mismatch';

export async function entryReloadFixture(page: Page, baseURL: string, scenario: ReloadScenario) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  const accepted: Launch = {
    ...recorded<Launch>('launches.json', 'launch-ready-create'),
    id: 'lch_original_receipt',
    state: 'resolving',
    revision: 2,
    event_head: 2,
    review: { needs: [], can_start_now: false },
  };
  const posts: {
    key?: string;
    body: string | null;
    status?: number;
    finished: boolean;
    failed: boolean;
  }[] = [];
  const requests = new Map<Request, (typeof posts)[number]>();
  page.on('requestfinished', (request) => {
    const post = requests.get(request);
    if (post !== undefined) post.finished = true;
  });
  page.on('requestfailed', (request) => {
    const post = requests.get(request);
    if (post !== undefined) post.failed = true;
  });
  page.on('response', (response) => {
    const post = requests.get(response.request());
    if (post !== undefined) post.status = response.status();
  });
  await page.addInitScript(() => {
    (window as unknown as { entryDocumentId: string }).entryDocumentId = crypto.randomUUID();
  });
  await page.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== baseURL) return route.fallback();
    const proof = verifiedEntryHeaders(request, 'local-owner');
    const reply = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        headers: proof,
        contentType: 'application/json',
        body: JSON.stringify(body),
      });
    if (url.pathname === '/auth/session')
      return reply({
        authenticated: true,
        subject: 'local-owner',
        display_name: 'Local owner',
        visitor: false,
        capabilities: ['use_own_sandboxes', 'launch_triggers'],
      });
    if (
      url.pathname === '/api/v2/triggers/public/public_contract/launches' &&
      request.method() === 'POST'
    ) {
      const post = {
        key: request.headers()['idempotency-key'],
        body: request.postData(),
        finished: false,
        failed: false,
      };
      posts.push(post);
      requests.set(request, post);
      if (posts.length === 1)
        return reply({ code: 'store_failed', title: 'Synthetic unknown result' }, 503);
      if (post.key !== posts[0]?.key || post.body !== posts[0]?.body)
        return reply(
          { code: 'idempotency_key_reused', title: 'Synthetic retained fingerprint mismatch' },
          422,
        );
      if (scenario === 'mismatch')
        return reply(
          { code: 'idempotency_key_reused', title: 'Synthetic retained fingerprint mismatch' },
          422,
        );
      if (scenario === 'receipt' || (scenario === 'pending' && posts.length >= 3))
        return reply(accepted, 201);
      return reply(
        {
          code: scenario === 'pending' ? 'idempotency_in_flight' : 'idempotency_recovery_required',
          title: 'Synthetic retained command',
        },
        409,
      );
    }
    if (url.pathname === '/api/v1/triggers/public/public_contract')
      return reply({
        ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
        always_review: true,
      });
    if (url.pathname === `/api/v1/launches/${accepted.id}`) return reply(accepted);
    if (url.pathname === `/api/v1/launches/${accepted.id}/requirements`)
      return reply(accepted.requirements);
    if (url.pathname === `/api/v1/agui/launches/${accepted.id}/events`)
      return route.fulfill({ contentType: 'text/event-stream', body: ': synthetic progress\n\n' });
    return route.fallback();
  });
  const stored = () => page.evaluate(() => sessionStorage.getItem('cma:public-intent'));
  return {
    accepted,
    posts,
    stored,
    packet: async () => JSON.parse((await stored()) ?? 'null') as EntryPacket | null,
    documentId: () =>
      page.evaluate(() => (window as unknown as { entryDocumentId: string }).entryDocumentId),
    waitPosts: async (count: number) => {
      await expect.poll(() => posts.length).toBe(count);
      await expect.poll(() => posts.every((post) => post.finished)).toBe(true);
      expect(posts.every((post) => !post.failed)).toBe(true);
    },
    finish: () => {
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.writes).toEqual([]);
    },
  };
}
