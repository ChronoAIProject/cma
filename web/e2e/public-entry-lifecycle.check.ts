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
        await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toHaveCount(0);
        flow.setSubject('different-owner');
        await page.evaluate(() => window.dispatchEvent(new Event('focus')));
        await expect.poll(() => flow.sessions.includes('different-owner')).toBe(true);
        await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
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
  test(`LOCAL Entry blocks B and ignores A's retired ${outcome} observer`, async ({
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
      await expect(page.getByRole('link', { name: 'Return to saved request' })).toHaveAttribute(
        'href',
        '/t/public_contract',
      );
      await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
      const retained = await flow.stored();
      const newer = await flow.packet();
      expect(newer).toEqual(original);
      expect(flow.posts).toHaveLength(1);
      await flow.release();
      expect(await flow.stored()).toBe(retained);
      expect(await flow.documentId()).toBe(documentId);
      await expect(page).toHaveURL(`${baseURL}/t/public_contract`);
      expect(flow.posts).toHaveLength(1);
      await page.screenshot({ path: info.outputPath(`entry-old-${outcome}.png`) });
      flow.setSubject('local-owner');
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      if (outcome !== 'accepted') {
        await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeEnabled();
        await page.getByRole('button', { name: 'Try again', exact: true }).click();
      }
      await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
      expect(flow.posts).toHaveLength(outcome === 'accepted' ? 1 : 3);
      for (const post of flow.posts) expect(post).toEqual(flow.posts[0]);
      flow.finish();
    } finally {
      flow.releaseGate();
    }
  });
}

for (const timing of ['pending', 'accepted'] as const) {
  for (const offset of [-1, 0, 1, -30 * 60 * 1000]) {
    test(`LOCAL same-owner Entry recovers ${timing} at expiry offset ${offset}`, async ({
      page,
      baseURL,
    }, info) => {
      await page.clock.setFixedTime(new Date('2026-11-23T00:10:00Z'));
      const flow = await entryFixture(page, baseURL!, 'command');
      try {
        await flow.prepare();
        await flow.waitHeld();
        const original = (await flow.packet())!;
        const retained = await flow.stored();
        const documentId = await flow.documentId();
        await page.goBack();
        await expect(page.getByRole('dialog')).toBeVisible();
        if (timing === 'accepted') {
          await flow.release();
          await expect(page).toHaveURL(`${baseURL}/launches/${flow.previous.id}`);
          expect(await flow.stored()).toBe(retained);
        }
        await page.clock.setFixedTime(new Date(original.expiresAt + offset));
        await page.getByRole('button', { name: 'Cancel', exact: true }).click();
        if (timing === 'pending') {
          await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
          expect(await flow.stored()).toBe(retained);
          expect(flow.posts).toHaveLength(1);
          await flow.release();
        }
        await expect(page).toHaveURL(`${baseURL}/launches/${flow.accepted.id}`);
        await expect(page.getByText('In progress', { exact: true })).toBeVisible();
        expect(await flow.stored()).toBeNull();
        expect(await flow.documentId()).toBe(documentId);
        expect(flow.posts).toHaveLength(1);
        expect(flow.posts[0]?.key).toBe(original.key);
        expect(JSON.parse(flow.posts[0]!.body!)).toEqual(original.body);
        await page.screenshot({ path: info.outputPath('recovered-original.png') });
        flow.finish();
      } finally {
        flow.releaseGate();
      }
    });
  }
}

for (const loss of ['reload', 'unknown', 'fenced'] as const) {
  test(`LOCAL expired Entry preserves its original request after ${loss}`, async ({
    page,
    baseURL,
  }, info) => {
    await page.clock.setFixedTime(new Date('2026-11-23T00:10:00Z'));
    const flow = await entryFixture(
      page,
      baseURL!,
      'command',
      loss === 'reload' ? 'accepted' : loss,
    );
    try {
      await flow.prepare();
      await flow.waitHeld();
      const original = (await flow.packet())!;
      const retained = await flow.stored();
      await page.goBack();
      await expect(page.getByRole('dialog')).toBeVisible();
      await flow.release();
      expect(await flow.stored()).toBe(retained);
      if (loss === 'reload') {
        await page.reload();
        await expect(page.getByRole('dialog')).toBeVisible();
      }
      await page.clock.setFixedTime(new Date(original.expiresAt + 1));
      await page.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(page.getByRole('alert')).toContainText('Your original request is saved');
      await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeDisabled();
      await expect(page.getByRole('button', { name: 'Try again', exact: true })).toBeDisabled();
      await expect(page).toHaveURL(`${baseURL}/t/public_contract`);
      expect(await flow.stored()).toBe(retained);
      expect(flow.posts).toHaveLength(1);
      expect(flow.posts[0]?.key).toBe(original.key);
      await page.screenshot({ path: info.outputPath('retained-original.png') });
      flow.finish();
    } finally {
      flow.releaseGate();
    }
  });
}
