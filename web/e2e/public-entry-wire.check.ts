import { verifiedEntryHeaders } from './public-entry-binding.fixture';
import { createHash } from 'node:crypto';
import { test, expect } from '@playwright/test';
import { entryReloadFixture } from './public-entry-reload.fixture';

const sha = (value: string) => createHash('sha256').update(value).digest('hex');
for (const variant of [
  'base',
  'placement',
  'referrer',
  'combined',
  'new',
  'resume',
  'visit-other-placement',
] as const) {
  test(`LOCAL Entry preserves exact wire bytes through reload: ${variant}`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await entryReloadFixture(page, baseURL!, 'receipt');
    const placement = !['base', 'referrer'].includes(variant);
    const referrer = ['referrer', 'combined', 'new', 'resume'].includes(variant);
    const choice = variant === 'new' || variant === 'resume' ? variant : undefined;
    let selectionKey: string | undefined;
    if (choice !== undefined) {
      await page.route('**/api/v2/triggers/public/public_contract/launches', async (route) => {
        if (selectionKey !== undefined) return route.fallback();
        selectionKey = route.request().headers()['idempotency-key'];
        await route.fulfill({
          status: 409,
          headers: verifiedEntryHeaders(route.request(), 'local-owner'),
          contentType: 'application/json',
          body: JSON.stringify({
            code: 'trigger_repeat_choice_required',
            title: 'Choose an agent',
            existing: {
              agent_id: 'agt_previous',
              title: 'Previous agent',
              created_at: '2026-11-23T00:00:00Z',
              first_message_excerpt: 'Synthetic work',
            },
          }),
        });
      });
    }
    const path = placement ? '/t/public_contract/placement_a' : '/t/public_contract';
    await page.goto(path, referrer ? { referer: `${baseURL}/publisher` } : undefined);
    await page.getByRole('button', { name: 'Try it', exact: true }).click();
    if (choice !== undefined)
      await page
        .getByRole('button', {
          name: choice === 'new' ? 'Start fresh' : 'Continue where you left off',
          exact: true,
        })
        .click();
    await flow.waitPosts(1);
    const original = (await flow.packet())!;
    const body = flow.posts[0]!.body!;
    expect(JSON.parse(body)).toEqual({
      start_if_ready: true,
      ...(placement ? { placement_id: 'placement_a' } : {}),
      ...(referrer ? { referrer_origin: baseURL } : {}),
      ...(choice ? { choice } : {}),
    });
    if (choice !== undefined) expect(original.key).not.toBe(selectionKey);
    const documentId = await flow.documentId();
    if (variant === 'visit-other-placement') {
      const retained = await flow.stored();
      await page.goto('/t/public_contract/placement_b');
      await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeEnabled();
      expect(await flow.stored()).toBe(retained);
      expect(flow.posts).toHaveLength(1);
      await page.goto(path);
    } else await page.reload();
    expect(await flow.documentId()).not.toBe(documentId);
    await flow.waitPosts(2);
    expect(flow.posts[1]?.key).toBe(original.key);
    expect(flow.posts[1]?.body).toBe(body);
    expect(sha(flow.posts[1]!.body!)).toBe(sha(body));
    expect(original.wireBody).toBe(body);
    await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
    expect(await flow.stored()).toBeNull();
    await info.attach('wire-byte-identity', {
      body: JSON.stringify({
        posts: flow.posts,
        fingerprints: flow.posts.map((post) => sha(post.body!)),
      }),
      contentType: 'application/json',
    });
    flow.finish();
  });
}

test('LOCAL Entry retains a fingerprint mismatch and stops dispatch after reload', async ({
  page,
  baseURL,
}, info) => {
  const flow = await entryReloadFixture(page, baseURL!, 'mismatch');
  await page.goto('/t/public_contract/placement_a', { referer: `${baseURL}/publisher` });
  await page.getByRole('button', { name: 'Try it', exact: true }).click();
  await flow.waitPosts(1);
  const original = (await flow.packet())!;
  await page.reload();
  await flow.waitPosts(2);
  expect(flow.posts[1]?.status).toBe(422);
  await expect(page.getByRole('alert')).toContainText('check the original result');
  await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
  expect(await flow.packet()).toEqual({ ...original, recoveryRequired: true });
  const retained = await flow.stored();
  await page.reload();
  await expect(page.getByRole('alert')).toContainText('original request is saved');
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
  expect(flow.posts).toHaveLength(2);
  expect(await flow.stored()).toBe(retained);
  await page.screenshot({
    path: info.outputPath('mismatch-preserved.png'),
    animations: 'disabled',
  });
  flow.finish();
});

test('LOCAL legacy submitted Entry without byte proof remains saved without dispatch', async ({
  page,
  baseURL,
}, info) => {
  const flow = await entryReloadFixture(page, baseURL!, 'receipt');
  await page.goto('/t/public_contract/placement_a');
  await page.getByRole('button', { name: 'Try it', exact: true }).click();
  await flow.waitPosts(1);
  // Model the earlier browser record format using the actual generated command.
  await page.evaluate(() => {
    const saved = JSON.parse(sessionStorage.getItem('cma:public-intent')!);
    delete saved.wireBody;
    sessionStorage.setItem('cma:public-intent', JSON.stringify(saved));
  });
  const retained = await flow.stored();
  await page.reload();
  await expect(page.getByRole('alert')).toContainText('original request is saved');
  await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
  expect(flow.posts).toHaveLength(1);
  expect(await flow.stored()).toBe(retained);
  await page.screenshot({ path: info.outputPath('legacy-retained.png'), animations: 'disabled' });
  flow.finish();
});
