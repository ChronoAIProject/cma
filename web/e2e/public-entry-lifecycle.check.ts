import { test, expect } from '@playwright/test';
import { entryFixture } from './public-entry-lifecycle.fixture';

test('LOCAL Entry fresh owner mismatch preserves the original recovery without dispatch', async ({
  page,
  baseURL,
}) => {
  const flow = await entryFixture(page, baseURL!, 'preflight');
  await flow.prepare();
  const original = await flow.packet();
  await flow.checkAccount();
  flow.setSubject('different-owner');
  await page.getByRole('button', { name: 'Try it', exact: true }).click();
  await expect(
    page.getByText('Sign in with the same CMA account that made this request.'),
  ).toBeVisible();
  const retained = await flow.packet();
  expect(retained).toMatchObject({
    id: original?.id,
    owner: 'local-owner',
    body: original?.body,
    credentialRecovery: 'retry',
  });
  expect(retained?.key).not.toBe(original?.key);
  expect(flow.posts).toHaveLength(1);
  flow.finish();
});

for (const exit of ['route', 'owner'] as const) {
  test(`LOCAL Entry cancels preflight after ${exit} exit without losing recovery`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await entryFixture(page, baseURL!, 'preflight');
    try {
      await flow.prepare();
      await flow.checkAccount();
      const documentId = await flow.documentId();
      flow.holdSession();
      await page.getByRole('button', { name: 'Try it', exact: true }).click();
      await flow.waitHeld();
      const retained = await flow.stored();
      if (exit === 'route') {
        await page.goBack();
        await expect(page.getByRole('dialog')).toBeVisible();
      } else {
        flow.setSubject('different-owner');
        await page.evaluate(() => window.dispatchEvent(new Event('focus')));
        await expect.poll(() => flow.sessions.includes('different-owner')).toBe(true);
        await expect(
          page.getByRole('button', { name: 'Check signed-in account', exact: true }),
        ).toHaveCount(0);
      }
      const expectedPath =
        exit === 'route' ? `/launches/${flow.previous.id}` : '/t/public_contract';
      await expect(page).toHaveURL(`${baseURL}${expectedPath}`);
      await flow.release();
      expect(flow.posts).toHaveLength(1);
      expect(await flow.stored()).toBe(retained);
      expect(await flow.documentId()).toBe(documentId);
      await expect(page).toHaveURL(`${baseURL}${expectedPath}`);
      await page.screenshot({ path: info.outputPath(`entry-${exit}-after-preflight.png`) });
      if (exit === 'owner') {
        await page.reload();
        await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
        await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
        expect(await flow.stored()).toBe(retained);
        expect(flow.posts).toHaveLength(1);
      }
      flow.finish();
    } finally {
      flow.releaseGate();
    }
  });
}

for (const outcome of ['accepted', 'credential-refusal', 'forbidden'] as const) {
  test(`LOCAL Entry ignores A's old ${outcome} while B recovers its own command`, async ({
    page,
    baseURL,
  }, info) => {
    const flow = await entryFixture(page, baseURL!, 'command', outcome);
    try {
      await flow.prepare();
      await flow.waitHeld();
      const original = await flow.packet();
      const documentId = await flow.documentId();
      flow.setSubject('different-owner');
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      await expect.poll(() => flow.sessions.includes('different-owner')).toBe(true);
      await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeEnabled();
      await page.getByRole('button', { name: 'Try it', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
      const retained = await flow.stored();
      const newer = await flow.packet();
      expect(newer?.id).not.toBe(original?.id);
      expect(newer?.key).not.toBe(original?.key);
      expect(newer?.owner).toBe('different-owner');
      expect(flow.posts).toHaveLength(2);
      await flow.release();
      expect(await flow.stored()).toBe(retained);
      expect(await flow.documentId()).toBe(documentId);
      await expect(page).toHaveURL(`${baseURL}/t/public_contract`);
      expect(flow.posts).toHaveLength(2);
      await page.screenshot({ path: info.outputPath(`entry-old-${outcome}.png`) });
      await page.getByRole('button', { name: 'Try again', exact: true }).click();
      await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
      expect(flow.posts).toHaveLength(3);
      expect(flow.posts[2]).toEqual(flow.posts[1]);
      flow.finish();
    } finally {
      flow.releaseGate();
    }
  });
}

test('LOCAL same-owner Entry re-entry observes the original in-flight command once', async ({
  page,
  baseURL,
}) => {
  const flow = await entryFixture(page, baseURL!, 'command');
  try {
    await flow.prepare();
    await flow.waitHeld();
    const retained = await flow.stored();
    await page.goBack();
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
    expect(await flow.stored()).toBe(retained);
    expect(flow.posts).toHaveLength(1);
    await flow.release();
    await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
    await expect(page.getByText('In progress', { exact: true })).toBeVisible();
    expect(await flow.stored()).toBeNull();
    expect(flow.posts).toHaveLength(1);
    flow.finish();
  } finally {
    flow.releaseGate();
  }
});
