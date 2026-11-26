import { test, expect, type Page } from '@playwright/test';
import { identityFixture, originalPath, ownerA, ownerB } from './public-entry-identity.fixture';

const start = (page: Page) => page.getByRole('button', { name: 'Try it', exact: true });
const retry = (page: Page) => page.getByRole('button', { name: 'Try again', exact: true });
async function unknown(page: Page, baseURL: string) {
  const flow = await identityFixture(page, baseURL);
  await page.goto(originalPath);
  await start(page).click();
  await flow.waitCount(1);
  await flow.authorizeOriginal();
  await start(page).click();
  await flow.waitCount(2);
  await expect(retry(page)).toBeEnabled();
  return flow;
}

test('LOCAL Entry rejects B after a delayed A preflight and recovers the exact A command after reload', async ({
  page,
  baseURL,
}, info) => {
  const flow = await unknown(page, baseURL!);
  try {
    const saved = await flow.stored();
    const packet = (await flow.packet())!;
    flow.holdNextOriginalSession();
    await retry(page).click();
    await flow.waitHeld();
    await flow.switchOwner(ownerB);
    flow.releaseHeld();
    await flow.waitCount(3);
    expect(flow.posts[2]).toMatchObject({
      cookieOwner: ownerB,
      expected: `v1.${Buffer.from(ownerA).toString('base64url')}`,
      key: packet.key,
      body: packet.wireBody,
      status: 409,
    });
    expect(flow.posts[2]?.path).toContain('/api/v2/');
    expect(flow.admissions()).toBe(2);
    expect(await flow.stored()).toBe(saved);
    await expect(page).toHaveURL(`${baseURL}${originalPath}`);
    await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
    await page.reload();
    expect(await flow.stored()).toBe(saved);
    expect(flow.posts).toHaveLength(3);
    await flow.switchOwner(ownerA);
    await page.reload();
    await flow.authorizeOriginal();
    await start(page).click();
    await flow.waitCount(4);
    expect(flow.posts[3]).toMatchObject({
      cookieOwner: ownerA,
      key: packet.key,
      body: packet.wireBody,
      status: 201,
    });
    await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
    expect(await flow.stored()).toBeNull();
  } finally {
    await flow.finish(info);
  }
});

test('LOCAL Entry binds its first command even when the cookie changes before submit', async ({
  page,
  baseURL,
}, info) => {
  const flow = await identityFixture(page, baseURL!, false);
  try {
    await page.goto(originalPath);
    await expect(start(page)).toBeEnabled();
    await flow.switchOwner(ownerB);
    await start(page).click();
    await flow.waitCount(1);
    expect(flow.admissions()).toBe(0);
    const saved = (await flow.packet())!;
    expect(saved.owner).toBe(ownerA);
    expect(flow.posts[0]).toMatchObject({ cookieOwner: ownerB, key: saved.key, status: 409 });
    await flow.switchOwner(ownerA);
    await page.reload();
    await flow.waitCount(2);
    expect(flow.posts[1]).toMatchObject({
      cookieOwner: ownerA,
      key: saved.key,
      body: saved.wireBody,
      status: 201,
    });
    await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
  } finally {
    await flow.finish(info);
  }
});

for (const status of [201, 409, 429, 404]) {
  test(`LOCAL Entry retains the exact unknown command on unverified ${status}`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await unknown(page, baseURL!);
    try {
      const saved = await flow.stored();
      const packet = (await flow.packet())!;
      flow.unverified(status);
      await retry(page).click();
      await flow.waitCount(3);
      await expect(page.getByRole('alert')).toContainText('could not verify');
      expect(await flow.stored()).toBe(saved);
      await expect(page).toHaveURL(`${baseURL}${originalPath}`);
      expect(flow.posts.every((post) => post.path.includes('/api/v2/'))).toBe(true);
      await retry(page).click();
      await flow.waitCount(4);
      expect(flow.posts[3]).toMatchObject({ key: packet.key, body: packet.wireBody, status: 201 });
      await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
    } finally {
      await flow.finish(info);
    }
  });
}

for (const boundary of ['wrong', 'anonymous', 'outage', 'unchanged']) {
  test(`LOCAL Entry preserves the session preflight control: ${boundary}`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await unknown(page, baseURL!);
    try {
      const saved = await flow.stored();
      const packet = (await flow.packet())!;
      if (boundary === 'wrong') await flow.switchOwner(ownerB);
      if (boundary === 'anonymous') await flow.switchOwner('anonymous');
      if (boundary === 'outage') flow.unavailable(true);
      await retry(page).click();
      if (boundary !== 'unchanged') {
        await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
        expect(flow.posts).toHaveLength(2);
        expect(await flow.stored()).toBe(saved);
        flow.unavailable(false);
        await flow.switchOwner(ownerA);
        await flow.authorizeOriginal();
        await start(page).click();
      }
      await flow.waitCount(3);
      expect(flow.posts[2]).toMatchObject({
        cookieOwner: ownerA,
        key: packet.key,
        body: packet.wireBody,
        status: 201,
      });
      await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
    } finally {
      await flow.finish(info);
    }
  });
}
