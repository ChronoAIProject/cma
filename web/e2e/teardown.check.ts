import { test, expect } from '@playwright/test';
import { teardownFixture } from './teardown.fixture';

for (const mode of [
  'complete',
  'held-get',
  'settled-before-deadline',
  'held-post',
  'missing-headers',
  'unreadable-post',
  'late-post',
] as const) {
  test(`LOCAL teardown retains uncertain ownership: ${mode}`, async ({ browser }) => {
    const completed = mode === 'complete' || mode === 'settled-before-deadline';
    const f = await teardownFixture();
    const h = f.harness(browser);
    const page = await h.open('visitor');
    const close = page.context().close.bind(page.context());
    let releaseClose = () => {};
    try {
      await f.own(page, h);
      const original = structuredClone(h.journal.resources);
      if (mode === 'complete') {
        await page.evaluate(async () => {
          await (await fetch('/api/v1/catalog')).text();
        });
      } else {
        await f.hold(
          page,
          mode === 'missing-headers'
            ? '/api/v1/held-no-headers'
            : ['held-post', 'unreadable-post', 'late-post'].includes(mode)
              ? '/api/v1/sandboxes'
              : '/api/v1/held',
          ['held-get', 'settled-before-deadline'].includes(mode) ? 'GET' : 'POST',
        );
      }
      if (mode === 'unreadable-post') {
        f.release('broken');
        await h.flush();
        expect(h.violations).toEqual(['response_inspection_failed']);
      }
      let closing!: () => void;
      const closeStarted = new Promise<void>((resolve) => {
        closing = resolve;
      });
      if (mode === 'late-post') {
        const gate = new Promise<void>((resolve) => {
          releaseClose = resolve;
        });
        page.context().close = async () => {
          closing();
          await gate;
          await close();
        };
      }
      const disposal = h.dispose(1_000);
      if (mode === 'settled-before-deadline') f.release();
      expect(h.dispose(10_000)).toBe(disposal);
      const outcome = disposal.then(
        () => 'passed',
        () => 'refused',
      );
      if (mode === 'late-post') {
        await closeStarted;
        f.release('"id":"late-sandbox"}');
        await h.flush();
        expect(h.journal.resources).toContainEqual({
          kind: 'sandboxes',
          id: 'late-sandbox',
          subject: f.subject,
        });
        releaseClose();
      }
      expect(await outcome).toBe(completed ? 'passed' : 'refused');
      expect(page.isClosed()).toBe(true);
      expect(h.journal.resources.slice(0, original.length)).toEqual(original);
      expect(f.rows().filter((r) => r.action === 'created').length).toBe(
        h.journal.resources.length,
      );
      if (completed) {
        expect(f.requests).toEqual(['GET', 'DELETE', 'GET']);
        expect(f.rows().map((r) => r.action)).toEqual([
          'created',
          'cleanup_accepted',
          'cleanup_complete',
        ]);
      } else {
        expect(f.requests).toEqual([]);
        expect(f.rows()).toContainEqual(
          expect.objectContaining({
            action: 'cleanup_blocked',
            reason: 'response_observation_incomplete',
          }),
        );
        for (const resource of h.journal.resources)
          expect(f.rows()).toContainEqual(
            expect.objectContaining({ action: 'cleanup_blocked', ...resource }),
          );
        expect(f.rows().some((r) => r.action === 'cleanup_complete')).toBe(false);
      }
      await expect(h.open('visitor')).rejects.toThrow('teardown has started');
      await expect(h.command(page, 'DELETE', '/api/v1/triggers/owned-trigger')).rejects.toThrow(
        'teardown has started',
      );
    } finally {
      releaseClose();
      f.release();
      await close();
      await f.close();
    }
  });
}

for (const fault of ['none', 'reject', 'stall'] as const) {
  test(`LOCAL teardown attempts every context release despite ${fault}`, async ({ browser }) => {
    const f = await teardownFixture();
    const h = f.harness(browser);
    const pages = [await h.open('visitor'), await h.open('visitor')];
    const originals = pages.map((page) => page.context().close.bind(page.context()));
    const attempts: number[] = [];
    try {
      await f.own(pages[0]!, h);
      await f.hold(pages[0]!);
      for (const [index, page] of pages.entries())
        page.context().close = async () => {
          attempts.push(index);
          if (index === 0 && fault === 'reject') throw Error('Synthetic close failure');
          if (index === 0 && fault === 'stall') await new Promise(() => {});
          await originals[index]!();
        };
      await expect(h.dispose(500)).rejects.toThrow('cleanup is incomplete');
      expect(attempts).toEqual([0, 1]);
      expect(pages[1]!.isClosed()).toBe(true);
      expect(pages[0]!.isClosed()).toBe(fault === 'none');
      expect(f.requests).toEqual([]);
      expect(f.rows().some((r) => r.action === 'cleanup_blocked')).toBe(true);
    } finally {
      f.release();
      await Promise.all(originals.map((close) => close()));
      await f.close();
    }
  });
}

test('LOCAL teardown retains unknown signup identity without inventing ownership', async ({
  browser,
}) => {
  const f = await teardownFixture();
  f.setAnonymous();
  const h = f.harness(browser);
  const page = await h.open('signup');
  try {
    await page.goto(f.origin);
    await f.hold(page, '/api/v1/sandboxes', 'POST');
    await expect(h.dispose(500)).rejects.toThrow('cleanup is incomplete');
    expect(page.isClosed()).toBe(true);
    expect(h.fixture.accounts.signup.subject).toBe('');
    expect(h.journal.resources).toEqual([]);
    expect(f.requests).toEqual([]);
    expect(f.rows()).toContainEqual(
      expect.objectContaining({
        action: 'cleanup_blocked',
        reason: 'response_observation_incomplete',
      }),
    );
    expect(f.rows().some((r) => r.action === 'created' || r.action === 'cleanup_complete')).toBe(
      false,
    );
    expect(h.violations).toContain('response_inspection_failed');
  } finally {
    f.release();
    await page.context().close();
    await f.close();
  }
});
