import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { LocalAgentV2, binding } from './agent-v2.fixture';

for (const phase of ['first', 'recovery'] as const) {
  test(`LOCAL Stop ${phase} dispatch with 100 ms boundary latency`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.loseControl = phase === 'recovery';
    try {
      await fixture.route(page, baseURL!);
      await page.route(/\/(api|auth)\//, async (route) => {
        await new Promise((resolve) => setTimeout(resolve, 100));
        await route.fallback();
      });
      await page.goto('/agents/agt_contract');
      const stop = page.getByRole('button', { name: 'Stop response', exact: true });
      await expect(stop).toBeEnabled();
      await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
      const action =
        phase === 'first'
          ? stop
          : page.getByRole('button', { name: 'Recover pending request', exact: true });
      if (phase === 'recovery') {
        await stop.click();
        await expect(action).toBeEnabled();
        expect(fixture.writes).toHaveLength(1);
      }
      // One browser clock separates the actual DOM click from Playwright's
      // actionability wait. The wrapper records route classes, never payloads.
      await page.evaluate(() => {
        const timing = { click: 0, requests: [] as { kind: string; start: number }[] };
        Object.assign(window, { stopDispatchTiming: timing });
        document.addEventListener(
          'click',
          (event) => {
            if ((event.target as Element).closest('button')) timing.click ||= performance.now();
          },
          { capture: true },
        );
        const original = window.fetch;
        window.fetch = (input, init) => {
          const url = new URL(input instanceof Request ? input.url : String(input), location.href);
          const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
          const kind =
            url.pathname === '/auth/session'
              ? 'identity'
              : url.pathname.endsWith('/controls')
                ? `controls-${method}`
                : undefined;
          if (timing.click && kind) timing.requests.push({ kind, start: performance.now() });
          return original(input, init);
        };
      });
      const receipt = page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname.endsWith('/controls'),
      );
      const invoked = performance.now();
      await action.click();
      const accepted = await receipt;
      const headersMs = performance.now() - invoked;
      await accepted.finished();
      const bodyMs = performance.now() - invoked;
      expect(accepted.status()).toBe(202);
      await expect(
        page.getByRole('button', { name: 'Recover pending request', exact: true }),
      ).toHaveCount(0);
      const timing = await page.evaluate(
        () =>
          Reflect.get(window, 'stopDispatchTiming') as {
            click: number;
            requests: { kind: string; start: number }[];
          },
      );
      const post = timing.requests.filter((row) => row.kind === 'controls-POST');
      expect(post).toHaveLength(1);
      expect(fixture.writes).toHaveLength(phase === 'first' ? 1 : 2);
      if (phase === 'recovery') expect(fixture.writes[1]).toEqual(fixture.writes[0]);
      // The accepted receipt is never proof that the original turn stopped.
      await expect(stop).toBeVisible();
      fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
      await expect(stop).toHaveCount(0);
      expect(fixture.errors).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      await writeFile(
        info.outputPath('timings.json'),
        JSON.stringify(
          {
            source: process.env.CMA_STOP_SOURCE ?? 'unspecified',
            phase,
            environment:
              'compiled Chrome loopback; 100 ms synthetic boundary delay; synthetic terminal; not production',
            domClickToDispatchMs: post[0]!.start - timing.click,
            invokedTo202HeadersMs: headersMs,
            invokedTo202BodyMs: bodyMs,
            requests: timing.requests.map((row) => ({
              kind: row.kind,
              afterClickMs: row.start - timing.click,
            })),
            controlPosts: fixture.writes.length,
          },
          null,
          2,
        ),
      );
    } finally {
      await fixture.close(page);
    }
  });
}
