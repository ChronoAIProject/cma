import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

const png = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAIAAAD8GO2jAAAACXBIWXMAAAABAAAAAQBPJcTWAAAAKklEQVR4nO3NQQ0AAAjEsCPBv2UwAb9OwFrJ5LN+vQMAAAAAAAAAAIDDFhnEAX2X/8JuAAAAAElFTkSuQmCC',
  'base64',
);
// A 200 ms blue frame generated with libvpx; no network media dependency.
const video = Buffer.from(
  'GkXfo59ChoEBQveBAULygQRC84EIQoKEd2VibUKHgQJChYECGFOAZwEAAAAAAAJgEU2bdLpNu4tTq4QVSalmU6yBoU27i1OrhBZUrmtTrIHYTbuMU6uEElTDZ1OsggElTbuMU6uEHFO7a1OsggJK7AEAAAAAAABZAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAVSalmsirXsYMPQkBNgI1MYXZmNjIuMTIuMTAxV0GNTGF2ZjYyLjEyLjEwMUSJiEBpAAAAAAAAFlSua8iuAQAAAAAAAD/XgQFzxYiJBM83e1SDw5yBACK1nIN1bmSIgQCGhVZfVlA4g4EBI+ODhAJiWgDgkLCBILqBIJqBAlWwhFW5gQESVMNn/HNzoGPAgGfImkWjh0VOQ09ERVJEh41MYXZmNjIuMTIuMTAxc3PWY8CLY8WIiQTPN3tUg8NnyKFFo4dFTkNPREVSRIeUTGF2YzYyLjI4LjEwMSBsaWJ2cHhnyKFFo4hEVVJBVElPTkSHkzAwOjAwOjAwLjIwMDAwMDAwMAAfQ7Z1QJ7ngQCjvYEAAIDQAgCdASogACAAAEcIhYWIhYSIAgICdaoD+AIIIQg9AP7/TRL//FhX8WFfxYV/8WFf/PzO7cX85gCjlYEAKACxAQAFEKwAGAAYWC/0AAhwAKOVgQBQALEBAAUQrAAYABhYL/QACHAAo5WBAHgAsQEABRCsABgAGFgv9AAIcACjlYEAoACxAQAFEKwAGAAYWC/0AAhwABxTu2uRu4+zgQC3iveBAfGCAabwgQM=',
  'base64',
);
const wav = Buffer.alloc(44 + 1600);
wav.write('RIFF');
wav.writeUInt32LE(wav.length - 8, 4);
wav.write('WAVEfmt ', 8);
wav.writeUInt32LE(16, 16);
wav.writeUInt16LE(1, 20);
wav.writeUInt16LE(1, 22);
wav.writeUInt32LE(8000, 24);
wav.writeUInt32LE(16000, 28);
wav.writeUInt16LE(2, 32);
wav.writeUInt16LE(16, 34);
wav.write('data', 36);
wav.writeUInt32LE(1600, 40);

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme} reply media loads through authorized files and timestamp survives reload`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    const finished = new Date(Date.now() - 2 * 86_400_000).toISOString();
    const started = new Date(Date.parse(finished) - 7_384_000).toISOString();
    const content = [
      'Results are ready.',
      '![Plot](sandbox:/home/cma/plot.png)',
      '[Recording](/home/cma/recording.wav)',
      '[Clip](/home/cma/clip.webm)',
      '![Retry image](/home/cma/retry.png)',
    ].join('\n\n');
    boundary.events = [
      { seq: 1, appended_at: started, kind: { type: 'turn_started', turn_sequence: 1 } },
      { seq: 2, appended_at: started, kind: { type: 'owner_message', content: 'Prepare media.' } },
      { seq: 3, appended_at: finished, kind: { type: 'agent_message', content } },
      { seq: 4, appended_at: finished, kind: { type: 'turn_completed', turn_sequence: 1 } },
    ];
    await boundary.route(page, baseURL!);
    await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
    await page.emulateMedia({ colorScheme: theme === 'dark' ? 'dark' : 'light' });
    const reads: string[] = [];
    let retry = true;
    await page.route('**/api/v1/agents/agt_contract/files/download?*', async (route) => {
      expect(route.request().method()).toBe('GET');
      const path = new URL(route.request().url()).searchParams.get('path')!;
      reads.push(path);
      if (path === 'cma/retry.png' && retry) {
        retry = false;
        return route.fulfill({ status: 503, json: { code: 'unavailable' } });
      }
      const body = path.endsWith('.webm') ? video : path.endsWith('.wav') ? wav : png;
      return route.fulfill({ contentType: 'application/octet-stream', body });
    });
    await page.route('**/api/v1/agents/agt_contract/transcript', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_contract',
          through_seq: 4,
          turns: [
            {
              turn_sequence: 1,
              started_at: started,
              owner_message: 'Prepare media.',
              agent_messages: [content],
              messages: [
                { sequence: 2, role: 'user', content: 'Prepare media.', appended_at: started },
                { sequence: 3, role: 'assistant', content, appended_at: finished },
              ],
              outcome: { type: 'completed', at: finished },
            },
          ],
        },
      }),
    );
    await page.goto('/agents/agt_contract');
    await expect(page.locator('html')).toHaveCSS('color-scheme', theme);
    await expect(page.getByText('Results are ready.')).toBeVisible();
    await page.locator('.agent-content__media').nth(0).scrollIntoViewIfNeeded();
    await expect(page.getByRole('img', { name: 'Plot', exact: true })).toBeVisible();
    await expect
      .poll(() =>
        page
          .getByRole('img', { name: 'Plot', exact: true })
          .evaluate((image: HTMLImageElement) => image.naturalWidth),
      )
      .toBe(32);
    await page.locator('.agent-content__media').nth(1).scrollIntoViewIfNeeded();
    await expect(page.locator('audio[aria-label="Recording"]')).toHaveAttribute('controls', '');
    await page.locator('.agent-content__media').nth(2).scrollIntoViewIfNeeded();
    const clip = page.locator('video[aria-label="Clip"]');
    await expect(clip).toHaveAttribute('controls', '');
    await expect
      .poll(() => clip.evaluate((media: HTMLVideoElement) => media.readyState))
      .toBeGreaterThanOrEqual(1);
    expect(await clip.evaluate((media: HTMLVideoElement) => media.paused && !media.autoplay)).toBe(
      true,
    );
    await page.locator('.agent-content__media').nth(3).scrollIntoViewIfNeeded();
    await page.getByRole('button', { name: 'Retry preview' }).click();
    await expect(page.getByRole('img', { name: 'Retry image' })).toBeVisible();
    const timestamp = page.locator('.message--assistant .message__timestamp');
    await expect(timestamp).toHaveAttribute('datetime', finished);
    await expect(timestamp).toHaveText(/^\d+ days? ago$/);
    await expect(page.getByLabel('Turn duration')).toHaveCount(0);
    expect(reads).toEqual(
      expect.arrayContaining([
        'cma/plot.png',
        'cma/recording.wav',
        'cma/clip.webm',
        'cma/retry.png',
      ]),
    );
    await page.screenshot({ path: info.outputPath('media-and-timestamp.png'), fullPage: true });
    await page.reload();
    await expect(timestamp).toHaveAttribute('datetime', finished);
    await expect(timestamp).toHaveText(/^\d+ days? ago$/);
    await expect(page.getByLabel('Turn duration')).toHaveCount(0);
    expect(boundary.errors).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.writes).toEqual([]);
  });
}
