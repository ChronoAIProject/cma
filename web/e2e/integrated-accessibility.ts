import { expect, type Browser, type Locator, type Page, type TestInfo } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

type Target = { name: string; width: number; height: number; expandedLabel: boolean };
type Result = {
  width: number;
  touch: boolean;
  media?: { coarse: boolean; hover: boolean; reduced: boolean };
  targets: Target[];
  failures: string[];
  errors: string[];
  completed: boolean;
  scroll?: { before: number; after: number };
};

async function target(locator: Locator, result: Result, name: string, label = false) {
  await expect(locator).toBeVisible();
  const rectangle = await locator.evaluate((element, useLabel) => {
    const effective = useLabel ? element.closest('label') : element;
    if (!effective) throw new Error('A native checkbox needs its effective labelled target');
    const bounds = effective.getBoundingClientRect();
    return { width: bounds.width, height: bounds.height };
  }, label);
  result.targets.push({ name, ...rectangle, expandedLabel: label });
}

async function composerVisible(page: Page, info: TestInfo, width: number, phase: string) {
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  await expect(composer).toBeVisible();
  const geometry = await composer.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const hit = document.elementFromPoint(
      bounds.x + bounds.width / 2,
      bounds.y + bounds.height / 2,
    );
    return {
      bounds: bounds.toJSON(),
      viewport: { width: innerWidth, height: innerHeight },
      hit: hit?.outerHTML.slice(0, 300),
      composerHit: element.contains(hit),
      visualViewport: {
        width: visualViewport?.width,
        height: visualViewport?.height,
        scale: visualViewport?.scale,
      },
    };
  });
  await writeFile(
    info.outputPath(`geometry-${phase}-${width}.json`),
    JSON.stringify(geometry, null, 2),
  );
  await page.screenshot({ path: info.outputPath(`layout-${phase}-${width}.png`), fullPage: true });
  await expect
    .poll(() =>
      composer.evaluate((element) => {
        const bounds = element.getBoundingClientRect();
        const horizontal = Math.max(0, Math.min(innerWidth - 1, bounds.x + bounds.width / 2));
        const vertical = Math.max(0, Math.min(innerHeight - 1, bounds.y + bounds.height / 2));
        return (
          bounds.top >= 0 &&
          bounds.bottom <= innerHeight &&
          element.contains(document.elementFromPoint(horizontal, vertical))
        );
      }),
    )
    .toBe(true);
}

async function inspect(page: Page, result: Result, info: TestInfo, agentId: string) {
  await page.goto('/');
  await page
    .getByLabel('Your task')
    .fill('Compare three tools with a deliberately long task description. '.repeat(12));
  await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: /^Cloud computer/ }).click();
  await expect(page.getByRole('menuitemcheckbox', { name: 'Agent browser' })).toBeEnabled();
  await expect(page.getByText('Checking task creation…', { exact: true })).not.toBeVisible();
  await writeFile(
    info.outputPath(`home-semantics-${result.width}.txt`),
    await page.locator('body').ariaSnapshot(),
  );
  await target(page.getByRole('button', { name: 'Start task', exact: true }), result, 'Start task');
  await target(
    page.getByRole('menuitemcheckbox', { name: 'Agent browser' }),
    result,
    'Agent browser',
  );
  await page.screenshot({ path: info.outputPath(`home-${result.width}.png`), fullPage: true });
  await page.goto(`/agents/${agentId}`);
  const answer = page.getByRole('article').filter({ hasText: 'The comparison is complete.' });
  const preview = answer.getByRole('button', { name: 'Preview report.txt', exact: true });
  await expect(preview).toBeEnabled();
  const composer = page.getByRole('textbox', { name: 'Message', exact: true });
  const draft =
    '保留我的草稿 — keep this unsent draft, including a long unbroken word: ' +
    'comparison'.repeat(200);
  await composer.fill(draft);
  await page.getByLabel('Choose attachments').setInputFiles({
    name: 'proof-notes.txt',
    mimeType: 'text/plain',
    buffer: Buffer.from('Retain this attachment while inspecting the actual answer.'),
  });
  const attachments = page.getByRole('list', { name: 'Attachments' });
  await expect(attachments).toContainText('Ready');
  await target(
    page.getByRole('button', { name: 'Attach files', exact: true }),
    result,
    'Attach files',
  );
  await target(
    page.getByRole('button', { name: 'Send message', exact: true }),
    result,
    'Send message',
  );
  await target(
    page.getByRole('button', { name: 'Remove proof-notes.txt', exact: true }),
    result,
    'Remove attachment',
  );
  await preview.scrollIntoViewIfNeeded();
  const scroller = page.locator('.agent-surface__conversation');
  const before = await scroller.evaluate((element) => element.scrollTop);
  await preview.focus();
  await page.keyboard.press('Enter');
  const panel = page.getByRole('complementary', { name: 'Deliverable preview' });
  await expect(panel).toContainText('Tool A');
  await expect(panel.getByRole('button', { name: 'Close preview' })).toBeFocused();
  await target(panel.getByRole('button', { name: 'Close preview' }), result, 'Close preview');
  await target(
    panel.getByRole('button', { name: 'Download report.txt' }),
    result,
    'Preview download',
  );
  await composerVisible(page, info, result.width, 'preview');
  await page.screenshot({ path: info.outputPath(`preview-${result.width}.png`), fullPage: true });
  await page.keyboard.press('Escape');
  await expect(panel).not.toBeVisible();
  await expect(preview).toBeFocused();
  const after = await scroller.evaluate((element) => element.scrollTop);
  result.scroll = { before, after };
  expect(
    Math.abs(after - before),
    'Closing preview retains the actual conversation scroll offset',
  ).toBeLessThanOrEqual(1);
  await expect(composer).toHaveValue(draft);
  await expect(attachments).toContainText('proof-notes.txt');
  await answer.hover();
  await answer.getByRole('button', { name: 'Edit reasons', exact: true }).click();
  const reasons = answer.getByRole('region', { name: 'Reasons for improvement' });
  await expect(reasons.getByRole('checkbox', { name: 'Incomplete answer' })).toBeChecked();
  for (const checkbox of await reasons.getByRole('checkbox').all())
    await target(
      checkbox,
      result,
      await checkbox.evaluate((element) => element.closest('label')!.textContent!.trim()),
      true,
    );
  await target(
    reasons.getByRole('button', { name: 'Save reasons', exact: true }),
    result,
    'Save reasons',
  );
  await target(
    reasons.getByRole('button', { name: 'Close', exact: true }),
    result,
    'Close reasons',
  );
  await composerVisible(page, info, result.width, 'reasons');
  await page.screenshot({ path: info.outputPath(`reasons-${result.width}.png`), fullPage: true });
  await reasons.getByRole('button', { name: 'Close', exact: true }).click();
  const edit = page.getByRole('button', { name: 'Edit suggestion 1 before sending' });
  await target(edit, result, 'Edit suggestion');
  await edit.click();
  const choice = page.getByRole('dialog', { name: 'Keep your draft?' });
  await expect(choice).toBeVisible();
  await expect(choice).toContainText('Your attachments stay in the composer.');
  await choice.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(composer).toHaveValue(draft);
  await edit.click();
  await choice.getByRole('button', { name: 'Replace text', exact: true }).click();
  await page.getByRole('button', { name: 'Undo replacement', exact: true }).click();
  await expect(composer).toBeFocused();
  await expect(composer).toHaveValue(draft);
  await expect(attachments).toContainText('Ready');
  await composerVisible(page, info, result.width, 'suggestions');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({
    path: info.outputPath(`conversation-${result.width}.png`),
    fullPage: true,
  });
  await writeFile(
    info.outputPath(`semantics-${result.width}.txt`),
    await page.locator('body').ariaSnapshot(),
  );
  result.completed = true;
}

export async function composedAccessibility(
  browser: Browser,
  origin: string,
  agentId: string,
  info: TestInfo,
  directory: string,
) {
  const results: Result[] = [];
  try {
    for (const width of [1440, 390, 320]) {
      const touch = width < 400;
      const result: Result = {
        width,
        touch,
        targets: [],
        failures: [],
        errors: [],
        completed: false,
      };
      results.push(result);
      const context = await browser.newContext({
        baseURL: origin,
        viewport: { width, height: touch ? 844 : 1000 },
        isMobile: touch,
        hasTouch: touch,
        reducedMotion: 'reduce',
      });
      try {
        const page = await context.newPage();
        page.on('pageerror', (error) => result.errors.push(error.message));
        page.on('response', (response) => {
          if (response.status() >= 400)
            result.failures.push(
              `${response.request().method()} ${new URL(response.url()).pathname} ${response.status()}`,
            );
        });
        result.media = await page.evaluate(() => ({
          coarse: matchMedia('(pointer: coarse)').matches,
          hover: matchMedia('(hover: hover)').matches,
          reduced: matchMedia('(prefers-reduced-motion: reduce)').matches,
        }));
        expect(result.media).toEqual({ coarse: touch, hover: !touch, reduced: true });
        await inspect(page, result, info, agentId);
      } finally {
        await context.close();
      }
    }
    for (const result of results) {
      expect(result.errors).toEqual([]);
      expect(result.failures).toEqual([]);
      if (result.touch)
        expect(
          result.targets.filter((value) => value.width < 44 || value.height < 44),
          'Actual effective touch targets are at least 44px in both dimensions',
        ).toEqual([]);
    }
  } finally {
    await writeFile(
      join(directory, 'accessibility-result.json'),
      JSON.stringify(
        {
          scope:
            'Actual current Home and same durable native answer; automated semantics, not a human screen-reader session',
          results,
        },
        null,
        2,
      ),
    );
  }
}
