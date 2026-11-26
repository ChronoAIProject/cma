import { verifiedEntryHeaders } from './public-entry-binding.fixture';
import { expect, type Page, type Request } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { recorded } from './agent-startup-fixtures';
import { entryReloadFixture } from './public-entry-reload.fixture';

export async function multirouteFixture(page: Page, baseURL: string, otherStatus: 201 | 503) {
  const original = await entryReloadFixture(page, baseURL, 'receipt');
  const accepted: Launch = { ...original.accepted, id: 'lch_other_entry' };
  const posts: { key?: string; body: string | null; finished: boolean; status?: number }[] = [];
  const requests = new Map<Request, (typeof posts)[number]>();
  page.on('requestfinished', (request) => {
    const post = requests.get(request);
    if (post !== undefined) post.finished = true;
  });
  page.on('response', (response) => {
    const post = requests.get(response.request());
    if (post !== undefined) post.status = response.status();
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
    if (url.pathname === '/api/v1/triggers/public/public_other')
      return reply({
        ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
        public_id: 'public_other',
        always_review: true,
      });
    if (
      url.pathname === '/api/v2/triggers/public/public_other/launches' &&
      request.method() === 'POST'
    ) {
      const post = {
        key: request.headers()['idempotency-key'],
        body: request.postData(),
        finished: false,
      };
      posts.push(post);
      requests.set(request, post);
      return otherStatus === 201
        ? reply(accepted, 201)
        : reply({ code: 'store_failed', title: 'Synthetic unknown result' }, 503);
    }
    if (url.pathname === `/api/v1/launches/${accepted.id}`) return reply(accepted);
    if (url.pathname === `/api/v1/launches/${accepted.id}/requirements`)
      return reply(accepted.requirements);
    if (url.pathname === `/api/v1/agui/launches/${accepted.id}/events`)
      return route.fulfill({ contentType: 'text/event-stream', body: ': synthetic progress\n\n' });
    return route.fallback();
  });
  return {
    original,
    accepted,
    posts,
    waitOther: async () => {
      await expect.poll(() => posts.length).toBe(1);
      await expect.poll(() => posts.every((post) => post.finished)).toBe(true);
      expect(posts[0]?.status).toBe(otherStatus);
    },
  };
}
