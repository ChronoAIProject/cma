import { mkdir, readFile } from 'node:fs/promises';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

const hash = 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad';
const first = {
  id: `delivery_${'a'.repeat(64)}`,
  reference: {
    agent_id: 'agt_contract',
    resource: { kind: 'live_file', path: 'result.txt' },
    name: 'result.txt',
    media_type: 'application/octet-stream',
    size_bytes: 3,
    sha256: hash,
  },
  status: 'unchecked',
};
const second = {
  id: `delivery_${'b'.repeat(64)}`,
  reference: {
    ...first.reference,
    resource: { kind: 'document', id: `doc_${'c'.repeat(32)}`, revision: 1 },
    name: 'A very long document title that wraps on a narrow mobile screen.md',
    media_type: 'text/markdown',
  },
  status: 'version_changed',
};
const marker = (item: typeof first | typeof second) =>
  `<!--cma-deliverable:v1:${Buffer.from(JSON.stringify(item.reference)).toString('base64url')}-->`;

for (const appearance of ['light', 'dark'])
  for (const width of [1440, 390]) {
    test(`LOCAL answer deliveries retain versions and save verified bytes at ${width}px in ${appearance}`, async ({
      page,
      baseURL,
    }) => {
      await page.setViewportSize({ width, height: 1000 });
      await page.addInitScript((mode) => localStorage.setItem('cma:appearance', mode), appearance);
      const boundary = new LocalStartup();
      const timestamp = '2026-11-03T01:00:00Z';
      boundary.events = [
        { seq: 1, appended_at: timestamp, kind: { type: 'turn_started', turn_sequence: 1 } },
        {
          seq: 2,
          appended_at: timestamp,
          kind: { type: 'owner_message', content: 'Deliver the file.' },
        },
        {
          seq: 3,
          appended_at: timestamp,
          kind: { type: 'agent_message', content: `The file is ready.\n${marker(first)}` },
        },
        { seq: 4, appended_at: timestamp, kind: { type: 'turn_completed', turn_sequence: 1 } },
        { seq: 5, appended_at: timestamp, kind: { type: 'turn_started', turn_sequence: 5 } },
        {
          seq: 6,
          appended_at: timestamp,
          kind: { type: 'owner_message', content: 'Now deliver a document.' },
        },
        {
          seq: 7,
          appended_at: timestamp,
          kind: { type: 'agent_message', content: `The document is ready.\n${marker(second)}` },
        },
        { seq: 8, appended_at: timestamp, kind: { type: 'turn_completed', turn_sequence: 5 } },
      ];
      await boundary.route(page, baseURL!);
      const requests: string[] = [];
      await page.route('**/api/v1/agents/agt_contract/answers/**', (route) => {
        const path = decodeURIComponent(new URL(route.request().url()).pathname);
        requests.push(path);
        const original = path.includes('event:3');
        const item = original ? first : second;
        const identity = {
          agent_id: 'agt_contract',
          answer_id: original ? 'event:3' : 'event:7',
          turn_sequence: original ? 1 : 5,
        };
        if (path.endsWith('/download'))
          return route.fulfill({ contentType: 'application/octet-stream', body: 'abc' });
        return route.fulfill({
          json: path.endsWith('/deliverables')
            ? { ...identity, items: [item] }
            : { ...identity, item },
        });
      });
      await page.goto('/agents/agt_contract');
      await expect(page.getByText('The document is ready.', { exact: true })).toBeVisible();
      const evidence = '/tmp/cma-ticket07-evidence';
      await mkdir(evidence, { recursive: true });
      if (process.env.CMA_DELIVERABLE_BEFORE === '1') {
        await page.screenshot({
          path: `${evidence}/before-${width}-${appearance}.png`,
          fullPage: true,
        });
        return;
      }
      const earlier = page.getByRole('article').filter({ hasText: 'The file is ready.' });
      const download = earlier.getByRole('button', { name: 'Download result.txt' });
      await expect(download).toBeEnabled();
      await expect(earlier.getByText(second.reference.name)).toHaveCount(0);
      await expect(
        page.getByText('Document has a newer revision; download keeps this revision'),
      ).toBeVisible();
      if (width === 390) expect((await download.boundingBox())!.height).toBeGreaterThanOrEqual(44);
      await download.focus();
      await expect(download).toBeFocused();
      const saved = page.waitForEvent('download');
      await page.keyboard.press('Enter');
      const file = await saved;
      expect(file.suggestedFilename()).toBe('result.txt');
      expect(await readFile((await file.path())!, 'utf8')).toBe('abc');
      expect(requests.filter((path) => path.endsWith(first.id))).toHaveLength(2);
      await page.screenshot({
        path: `${evidence}/after-${width}-${appearance}.png`,
        fullPage: true,
      });
      await page.reload();
      await expect(download).toBeEnabled();
      await expect(earlier.getByText(second.reference.name)).toHaveCount(0);
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }

test('LOCAL long history fetches visible references only and keeps the reader still when metadata arrives', async ({
  page,
  baseURL,
}) => {
  await page.setViewportSize({ width: 390, height: 1000 });
  const boundary = new LocalStartup();
  const timestamp = '2026-11-03T01:00:00Z';
  boundary.events = Array.from({ length: 80 }, (_, index) => {
    const sequence = index * 4 + 1;
    return [
      {
        seq: sequence,
        appended_at: timestamp,
        kind: { type: 'turn_started', turn_sequence: sequence },
      },
      {
        seq: sequence + 1,
        appended_at: timestamp,
        kind: { type: 'owner_message', content: `Task ${index}` },
      },
      {
        seq: sequence + 2,
        appended_at: timestamp,
        kind: {
          type: 'agent_message',
          content: `Recorded answer ${index}.\n\n${'A durable result stays with its own answer. '.repeat(4)}\n${index % 10 === 0 ? marker(second) : ''}`,
        },
      },
      {
        seq: sequence + 3,
        appended_at: timestamp,
        kind: { type: 'turn_completed', turn_sequence: sequence },
      },
    ];
  }).flat();
  await boundary.route(page, baseURL!);
  const requests: string[] = [];
  let release: (() => void) | undefined;
  await page.route('**/api/v1/agents/agt_contract/answers/**', async (route) => {
    const path = decodeURIComponent(new URL(route.request().url()).pathname);
    requests.push(path);
    await new Promise<void>((resolve) => {
      release = resolve;
    });
    const answer = /answers\/(event:\d+)/.exec(path)![1]!;
    await route.fulfill({
      json: {
        agent_id: 'agt_contract',
        answer_id: answer,
        turn_sequence: Number(answer.slice(6)) - 2,
        items: [second],
      },
    });
  });
  await page.goto('/agents/agt_contract');
  await expect(page.getByText('Recorded answer 79.', { exact: true })).toBeVisible();
  expect(requests).toHaveLength(0);
  const anchor = page.getByText('Recorded answer 41.', { exact: true });
  const pending = page
    .getByRole('article')
    .filter({ hasText: 'Recorded answer 40.' })
    .locator('.answer-deliveries');
  await pending.evaluate((element) => {
    const scroller = element.closest('.agent-surface__conversation')!;
    scroller.scrollTop +=
      element.getBoundingClientRect().top - scroller.getBoundingClientRect().top - 8;
  });
  await expect.poll(() => requests.length).toBe(1);
  await expect(anchor).toBeVisible();
  const before = (await anchor.boundingBox())!.y;
  release!();
  await expect(
    pending.getByRole('button', { name: `Download ${second.reference.name}` }),
  ).toBeVisible();
  await expect
    .poll(async () => Math.abs((await anchor.boundingBox())!.y - before))
    .toBeLessThanOrEqual(2);
  expect(requests).toHaveLength(1);
});
