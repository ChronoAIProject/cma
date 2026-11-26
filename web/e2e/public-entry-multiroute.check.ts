import { test, expect } from '@playwright/test';
import { multirouteFixture } from './public-entry-multiroute.fixture';

for (const otherStatus of [201, 503] as const) {
  test(`LOCAL multi-entry keeps A until recovery before B can return ${otherStatus}`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await multirouteFixture(page, baseURL!, otherStatus);
    await page.goto('/t/public_contract');
    await page.getByRole('button', { name: 'Try it', exact: true }).click();
    await flow.original.waitPosts(1);
    const original = (await flow.original.packet())!;
    const bytes = await flow.original.stored();
    const firstDocument = await flow.original.documentId();
    await page.goto('/t/public_other');
    expect(await flow.original.documentId()).not.toBe(firstDocument);
    await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeEnabled();
    expect(await flow.original.stored()).toBe(bytes);
    expect(flow.posts).toEqual([]);
    await page.getByRole('button', { name: 'Try it', exact: true }).click();
    await expect(page.getByRole('alert')).toContainText('earlier request');
    await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
    expect(flow.posts).toEqual([]);
    expect(await flow.original.stored()).toBe(bytes);
    await page.screenshot({ path: info.outputPath('other-entry-blocked.png') });
    await page.reload();
    await page.getByRole('button', { name: 'Try it', exact: true }).click();
    await expect(page.getByRole('link', { name: 'Return to saved request' })).toHaveAttribute(
      'href',
      '/t/public_contract',
    );
    expect(flow.posts).toEqual([]);
    expect(await flow.original.stored()).toBe(bytes);
    await page.getByRole('link', { name: 'Return to saved request' }).click();
    await flow.original.waitPosts(2);
    await expect(page).toHaveURL(`${baseURL}/launches/${flow.original.accepted.id}`);
    expect(flow.original.posts[1]?.key).toBe(original.key);
    expect(flow.original.posts[1]?.body).toBe(flow.original.posts[0]?.body);
    expect(await flow.original.stored()).toBeNull();
    await page.goto('/t/public_other');
    await page.getByRole('button', { name: 'Try it', exact: true }).click();
    await flow.waitOther();
    expect(flow.posts[0]?.key).not.toBe(original.key);
    if (otherStatus === 201) {
      await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
      expect(await flow.original.stored()).toBeNull();
    } else {
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
      expect(await flow.original.packet()).toMatchObject({
        publicId: 'public_other',
        key: flow.posts[0]?.key,
        submitted: true,
      });
    }
    await page.screenshot({ path: info.outputPath('other-entry-after-recovery.png') });
    await info.attach('wire-requests', {
      body: JSON.stringify({ a: flow.original.posts, b: flow.posts }),
      contentType: 'application/json',
    });
    flow.original.finish();
  });
}

test('LOCAL expired A blocks B and remains fenced after following its recovery link', async ({
  page,
  baseURL,
}, info) => {
  const flow = await multirouteFixture(page, baseURL!, 201);
  await page.clock.setFixedTime(new Date('2026-11-23T01:00:00Z'));
  await page.goto('/t/public_contract/placement_a');
  await page.getByRole('button', { name: 'Try it', exact: true }).click();
  await flow.original.waitPosts(1);
  const original = (await flow.original.packet())!;
  const bytes = await flow.original.stored();
  await page.clock.setFixedTime(new Date(original.expiresAt + 1));
  await page.goto('/t/public_contract/placement_b');
  await page.getByRole('button', { name: 'Try it', exact: true }).click();
  await expect(page.getByRole('link', { name: 'Return to saved request' })).toHaveAttribute(
    'href',
    '/t/public_contract/placement_a',
  );
  expect(await flow.original.stored()).toBe(bytes);
  await page.getByRole('link', { name: 'Return to saved request' }).click();
  await expect(page.getByRole('alert')).toContainText('original request is saved');
  await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
  expect(flow.original.posts).toHaveLength(1);
  expect(flow.posts).toEqual([]);
  expect(await flow.original.stored()).toBe(bytes);
  await page.screenshot({ path: info.outputPath('expired-original-preserved.png') });
  flow.original.finish();
});

test('LOCAL independent tabs retain their own unresolved entries', async ({
  page,
  context,
  baseURL,
}, info) => {
  const flow = await multirouteFixture(page, baseURL!, 503);
  await page.goto('/t/public_contract');
  await page.getByRole('button', { name: 'Try it', exact: true }).click();
  await flow.original.waitPosts(1);
  const original = (await flow.original.packet())!;
  const other = await context.newPage();
  try {
    const independent = await multirouteFixture(other, baseURL!, 503);
    await other.goto('/t/public_other');
    await other.getByRole('button', { name: 'Try it', exact: true }).click();
    await independent.waitOther();
    const otherBytes = await independent.original.stored();
    expect((await independent.original.packet())?.key).not.toBe(original.key);
    await page.reload();
    await flow.original.waitPosts(2);
    await expect(page).toHaveURL(`${baseURL}/launches/${flow.original.accepted.id}`);
    expect(await flow.original.stored()).toBeNull();
    expect(await independent.original.stored()).toBe(otherBytes);
    await other.screenshot({ path: info.outputPath('independent-entry-preserved.png') });
    flow.original.finish();
    independent.original.finish();
  } finally {
    await other.close();
  }
});
