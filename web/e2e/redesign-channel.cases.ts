import { test, expect } from '@playwright/test';
import { writeFileSync } from 'node:fs';
import { activityPage, receipt, responseConsumer, setupActivity } from './redesign-channel.fixture';

export function registerRedesignChannelCases() {
  for (const theme of ['dark', 'light'] as const) {
    for (const size of [
      { width: 1440, height: 900, scale: 1, suffix: '', label: 'desktop' },
      { width: 390, height: 844, scale: 1, suffix: '-narrow', label: 'narrow' },
      { width: 720, height: 450, scale: 2, suffix: '-zoom-200', label: '200% reflow' },
    ]) {
      test.describe(`${theme} ${size.label}`, () => {
        test.use({
          viewport: { width: size.width, height: size.height },
          deviceScaleFactor: size.scale,
        });
        test('LOCAL Activity receipt facts and keyboard access', async ({
          page,
          baseURL,
        }, info) => {
          const { state, view, boundary, browserErrors } = await setupActivity(
            page,
            baseURL!,
            theme,
          );
          await expect(view.getByText('Agent response produced')).toBeVisible();
          await expect(view.getByText('Delivery not confirmed')).toBeVisible();
          await expect(view.getByText(/This doesn’t mean the reply wasn’t sent/)).toBeVisible();
          await expect(view.getByText('Delivery confirmed', { exact: true })).toHaveCount(0);
          if (theme === 'dark')
            await expect(page.locator('body')).toHaveAttribute('data-ds-dark-theme');
          else await expect(page.locator('body')).not.toHaveAttribute('data-ds-dark-theme');
          await view
            .getByRole('heading', { name: 'Conversation activity' })
            .scrollIntoViewIfNeeded();
          await expect(view.getByRole('button', { name: 'Refresh activity' })).toBeVisible();
          await expect
            .poll(() => view.evaluate((element) => element.scrollWidth <= element.clientWidth + 1))
            .toBe(true);
          await expect
            .poll(() =>
              page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
            )
            .toBe(true);
          const file = `18-channel-${theme}${size.suffix}.png`;
          await page.screenshot({ path: info.outputPath(file), animations: 'disabled' });
          await view.getByText('Delivery not confirmed').scrollIntoViewIfNeeded();
          const receiptsFile = `18-channel-${theme}${size.suffix}-receipts.png`;
          await page.screenshot({ path: info.outputPath(receiptsFile), animations: 'disabled' });
          const disclosure = view.getByText('Response details', { exact: true });
          await disclosure.focus();
          await page.keyboard.press('Enter');
          await expect(view.getByText('agt_sample')).toBeVisible();
          await expect(disclosure).toBeFocused();
          await page.keyboard.press('Escape');
          await expect(page.getByRole('dialog')).toHaveCount(0);
          await expect(
            page
              .getByRole('region', { name: 'Delivery map' })
              .getByRole('button', { name: 'Activity', exact: true }),
          ).toBeFocused();
          expect(state.writes).toEqual([]);
          expect(browserErrors).toEqual([]);
          expect(boundary.errors).toEqual([]);
          expect(boundary.unexpected).toEqual([]);
          writeFileSync(
            info.outputPath('screenshots.json'),
            JSON.stringify(
              {
                files: [file, receiptsFile],
                theme,
                ...size,
                route: '/event-gateway',
                host: 'Channel settings → Activity',
                fixture: 'response produced, no delivery receipt',
                build: await (await page.request.get('/build-info.json')).json(),
              },
              null,
              2,
            ),
          );
        });
      });
    }
  }

  test('LOCAL Activity fixture records uncaught errors and rejects their telemetry', async ({
    page,
    baseURL,
  }) => {
    const { browserErrors, boundary, state } = await setupActivity(page, baseURL!);
    await page.evaluate(() => {
      const script = document.createElement('script');
      script.textContent = 'throw new Error("Synthetic Activity failure")';
      document.head.append(script);
    });
    await expect
      .poll(() => browserErrors)
      .toEqual([
        expect.objectContaining({
          type: 'error',
          message: 'Uncaught Error: Synthetic Activity failure',
          stack: expect.stringContaining('Synthetic Activity failure'),
        }),
      ]);
    await expect.poll(() => boundary.errors).toEqual(['Synthetic Activity failure']);
    await expect.poll(() => boundary.unexpected).toEqual(['POST /api/v1/telemetry/browser']);
    expect(state.writes).toEqual([]);
  });

  test('LOCAL Activity refresh preserves separate consumer and recipient facts with no outbound mutation', async ({
    page,
    baseURL,
  }) => {
    const { state, view, boundary } = await setupActivity(page, baseURL!);
    const initialReads = state.reads.length;
    state.snapshot = activityPage([
      { ...responseConsumer, deliveries: [receipt('confirmed'), receipt('unknown', 'other-team')] },
      {
        ...responseConsumer,
        consumer_id: 'helper',
        display_name: 'Sample helper',
        produced_at: null,
        state: 'failed',
        deliveries: [receipt('rejected', 'helper-team')],
      },
    ]);
    state.snapshot.has_more = true;
    let release!: () => void;
    state.hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    await view.getByRole('button', { name: 'Refresh activity' }).click();
    await expect.poll(() => state.reads.length).toBe(initialReads + 1);
    await expect(view.getByRole('button', { name: 'Refresh activity' })).toBeDisabled();
    await expect(view.getByText('Delivery not confirmed')).toBeVisible();
    release();
    state.hold = undefined;
    const first = view.getByRole('region', { name: 'Response and delivery for Sample task' });
    const second = view.getByRole('region', { name: 'Response and delivery for Sample helper' });
    await expect(first.getByText('Delivery confirmed', { exact: true })).toBeVisible();
    await expect(first.getByText('unknown', { exact: true })).toBeVisible();
    await expect(first.getByText('lark_chat: other-team')).toBeVisible();
    await expect(second.getByText('Agent response not confirmed')).toBeVisible();
    await expect(second.getByText('rejected', { exact: true })).toBeVisible();
    await expect(second.getByText('Delivery confirmed')).toHaveCount(0);
    await expect(
      view.getByText('Showing 1 recent events (up to 50; retained for 7 days).'),
    ).toBeVisible();
    await expect(view.getByText('Live activity', { exact: true })).toBeVisible();
    await page.clock.runFor(60_000);
    expect(state.reads).toHaveLength(initialReads + 1);
    expect(new Set(state.reads)).toEqual(
      new Set(['/api/event-gateway/v1/channels/channel-local/activity/stream']),
    );
    expect(state.writes).toEqual([]);
    expect(boundary.writes).toEqual([]);
  });

  test('LOCAL Activity read refusal retains prior facts and a channel switch clears them', async ({
    page,
    baseURL,
  }) => {
    const { state, view } = await setupActivity(page, baseURL!);
    state.status = 403;
    await view.getByRole('button', { name: 'Refresh activity' }).click();
    await expect(view.getByText('Live activity disconnected')).toBeVisible();
    await expect(view.getByText('demo-event-01')).toBeVisible();
    const afterRefusal = state.reads.length;
    await page.clock.runFor(60_000);
    expect(state.reads).toHaveLength(afterRefusal);
    await page.keyboard.press('Escape');
    await page
      .getByRole('region', { name: 'Channels', exact: true })
      .getByRole('treeitem', { name: /^Other team/ })
      .click();
    await page
      .getByRole('region', { name: 'Delivery map' })
      .getByRole('button', { name: 'Activity', exact: true })
      .click();
    await expect(view.getByText(/Activity records haven’t loaded/)).toBeVisible();
    await expect(view.getByText('demo-event-01')).toHaveCount(0);
    await expect(view.getByText(/No events recorded/)).toHaveCount(0);
    state.status = 200;
    state.snapshot = { ...activityPage(), items: [] };
    await view.getByRole('button', { name: 'Refresh activity' }).click();
    await expect(view.getByText('No events recorded in the last 7 days.')).toBeVisible();
    expect(state.reads.at(-1)).toBe('/api/event-gateway/v1/channels/other-channel/activity/stream');
    expect(state.writes).toEqual([]);
  });
}
