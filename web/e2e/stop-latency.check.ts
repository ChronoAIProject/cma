import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { LocalAgentV2, binding } from './agent-v2.fixture';

test('LOCAL Stop passes fresh identity barriers while a background identity read is held', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalAgentV2();
  fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
  const baseline = process.env.CMA_STOP_BASELINE === '1';
  let release!: () => void;
  let blocked!: () => void;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const started = new Promise<void>((resolve) => {
    blocked = resolve;
  });
  let intercept = true;
  let released = false;
  let releaseTimer: ReturnType<typeof setTimeout> | undefined;
  const marks: Record<string, number> = {};
  let identities = 0;
  let heldIdentities = 0;
  try {
    await fixture.route(page, baseURL!);
    await page.goto('/agents/agt_contract');
    const stop = page.getByRole('button', { name: 'Stop response', exact: true });
    await expect(stop).toBeEnabled();
    await expect(page.locator('.agent-surface__history-notice')).toHaveCount(0);
    await page.route('**/auth/session', async (route) => {
      identities += 1;
      if (intercept) {
        heldIdentities += 1;
        marks.backgroundStarted = performance.now();
        if (heldIdentities >= 2) blocked();
        await held;
      }
      await route.fallback();
    });
    // Focus refreshes application identity as well as transport observers.
    // Hold both requests so an independent application check cannot be mistaken
    // for the transport gate whose scheduling this regression exercises.
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await started;
    intercept = false;
    const receipt = page.waitForResponse(
      (response) =>
        response.request().method() === 'POST' &&
        new URL(response.url()).pathname.endsWith('/controls'),
    );
    marks.clickInvoked = performance.now();
    if (baseline)
      releaseTimer = setTimeout(() => {
        released = true;
        marks.backgroundReleased = performance.now();
        release();
      }, 1000);
    await stop.click();
    marks.clickReturned = performance.now();
    const accepted = await receipt;
    marks.receiptHeaders = performance.now();
    await accepted.finished();
    marks.receiptBody = performance.now();
    expect(accepted.status()).toBe(202);
    expect(released).toBe(baseline);
    expect(fixture.writes).toHaveLength(1);
    expect(fixture.writes[0]!.body.command).toEqual({ type: 'cancel' });
    // A delivered control still needs the original turn's durable terminal.
    await expect(stop).toBeVisible();
    await expect(
      page.getByRole('region', { name: 'What the agent is working through' }),
    ).toBeVisible();
    if (!released) {
      released = true;
      marks.backgroundReleased = performance.now();
      release();
    }
    fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
    await expect(stop).toHaveCount(0);
    marks.syntheticTerminalReflected = performance.now();
    expect(fixture.errors).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
    await writeFile(
      info.outputPath('timings.json'),
      JSON.stringify(
        {
          source: process.env.CMA_STOP_SOURCE ?? 'unspecified',
          environment:
            'compiled loopback; held background identity; synthetic terminal after receipt; not production',
          baseline,
          marks,
          clickTo202Ms: marks.receiptHeaders! - marks.clickInvoked!,
          identityRequests: identities,
          heldIdentityRequests: heldIdentities,
          controlPosts: fixture.writes.length,
          backgroundReleasedBefore202: baseline,
        },
        null,
        2,
      ),
    );
  } finally {
    clearTimeout(releaseTimer);
    release();
    await fixture.close(page);
  }
});
