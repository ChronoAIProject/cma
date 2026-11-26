import { test, expect } from '@playwright/test';
import { entryReloadFixture } from './public-entry-reload.fixture';

for (const scenario of ['receipt', 'pending', 'recovery-required', 'expired-fence'] as const) {
  test(`LOCAL Entry keeps its original command across reload: ${scenario}`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await entryReloadFixture(page, baseURL!, scenario);
    await page.clock.setFixedTime(new Date('2026-11-23T01:00:00Z'));
    await page.goto('/t/public_contract');
    await page.getByRole('button', { name: 'Try it', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
    await flow.waitPosts(1);
    const original = (await flow.packet())!;
    expect(original).toMatchObject({ owner: 'local-owner', submitted: true });
    expect(original.key).toBe(flow.posts[0]?.key);
    expect(flow.posts[0]?.status).toBe(503);
    expect(JSON.parse(flow.posts[0]!.body!)).toEqual(original.body);
    const firstDocument = await flow.documentId();
    await page.clock.setFixedTime(new Date(original.expiresAt - 1));
    await page.reload();
    expect(await flow.documentId()).not.toBe(firstDocument);
    await flow.waitPosts(2);
    if (scenario === 'receipt') {
      await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
      expect(flow.posts[1]?.status).toBe(201);
      expect(await flow.stored()).toBeNull();
    } else {
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
      await expect(page.getByRole('alert')).toContainText(
        scenario === 'pending' ? 'still processing' : 'Contact your CMA administrator',
      );
      expect(flow.posts[1]?.status).toBe(409);
      expect(await flow.packet()).toEqual(original);
      const currentDocument = await flow.documentId();
      if (scenario === 'pending') {
        await page.getByRole('button', { name: 'Try again', exact: true }).click();
        await flow.waitPosts(3);
        await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
        expect(flow.posts[2]?.status).toBe(201);
        expect(await flow.stored()).toBeNull();
      } else if (scenario === 'recovery-required') {
        await page.reload();
        expect(await flow.documentId()).not.toBe(currentDocument);
        await flow.waitPosts(3);
        await page.getByRole('button', { name: 'Try again', exact: true }).click();
        await flow.waitPosts(4);
        await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
        expect(flow.posts.slice(1).map((post) => post.status)).toEqual([409, 409, 409]);
        expect(await flow.packet()).toEqual(original);
      } else {
        const retained = await flow.stored();
        await page.clock.setFixedTime(new Date(original.expiresAt + 1));
        await page.reload();
        expect(await flow.documentId()).not.toBe(currentDocument);
        expect(await page.evaluate(() => Date.now())).toBe(original.expiresAt + 1);
        await expect(page.getByRole('alert')).toContainText('Your original request is saved');
        await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
        await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
        expect(flow.posts).toHaveLength(2);
        expect(await flow.stored()).toBe(retained);
        expect(await flow.packet()).toEqual(original);
        await expect(page).toHaveURL(`${baseURL}/t/public_contract`);
      }
    }
    expect(flow.posts.every((post) => post.key === original.key)).toBe(true);
    expect(flow.posts.every((post) => post.body === flow.posts[0]?.body)).toBe(true);
    await page.screenshot({ path: info.outputPath('entry-reload-result.png') });
    await info.attach('wire-requests', {
      body: JSON.stringify(flow.posts),
      contentType: 'application/json',
    });
    flow.finish();
  });
}
