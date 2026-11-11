import { expect, test } from '@playwright/test';
import { createHash } from 'node:crypto';
import { readFile, rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { composedControl } from './integrated-control';
import { observeJourney, sanitizedBody } from './integrated-observation';
import { composedAccessibility } from './integrated-accessibility';
import { reconnectRunningTurn } from './integrated-reconnect';
import { retainedAssets } from './integrated-compatibility';
import { authorityBoundaries } from './integrated-authority';

test('integrated task file feedback and edited follow-up survive reload with exact ownership', async ({
  page,
}, info) => {
  const directory = process.env.CMA_DESKTOP_RELAY_DIRECTORY!;
  const ready = JSON.parse(await readFile(join(directory, 'public-ready.json'), 'utf8'));
  const observation = observeJourney(page);
  const errors: string[] = [];
  const failures: { method: string; path: string; status: number; body?: string; at: number }[] =
    [];
  const failureReads: Promise<void>[] = [];
  const writes: { path: string; body: unknown; key?: string }[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('response', (response) => {
    if (response.status() >= 400)
      failureReads.push(
        response
          .text()
          .catch(() => 'Response body unavailable')
          .then((body) => {
            failures.push({
              method: response.request().method(),
              path: new URL(response.url()).pathname,
              status: response.status(),
              body,
              at: Date.now(),
            });
          }),
      );
  });
  page.on('request', (request) => {
    if (['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method()))
      writes.push({
        path: new URL(request.url()).pathname,
        body: sanitizedBody(request),
        key: request.headers()['idempotency-key'],
      });
  });
  let creationCount = 0;
  try {
    await page.route(/\/api\/(agent-manager\/)?v1\/agents$/, async (route) => {
      if (route.request().method() !== 'POST') return route.continue();
      const response = await route.fetch();
      expect(response.status()).toBe(202);
      const receipt = await response.json();
      const record = {
        body: route.request().postDataJSON(),
        key: route.request().headers()['idempotency-key'],
        receipt,
      };
      expect(record.body).toEqual(ready.body);
      creationCount += 1;
      if (creationCount === 1) {
        observation.mark('firstReceipt');
        await writeFile(join(directory, 'creation.tmp'), JSON.stringify(record));
        await rename(join(directory, 'creation.tmp'), join(directory, 'creation.json'));
        return route.abort('failed');
      }
      await route.fulfill({ response });
    });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/');
    // This fixture retains the original desktop-enabled request for recovery.
    const identity = await page.request.get('/auth/session');
    const { subject } = await identity.json();
    expect(typeof subject).toBe('string');
    await page.evaluate(
      ({ body, owner }) => {
        sessionStorage.setItem(
          `cma:agent-creation:${JSON.stringify([location.origin, owner])}`,
          JSON.stringify({ version: 1, intent: { key: crypto.randomUUID(), body } }),
        );
      },
      { body: ready.body, owner: subject },
    );
    await page.reload();
    await page.getByRole('button', { name: /^Cloud computer/ }).click();
    const browserChoice = page.getByRole('menuitemcheckbox', { name: 'Agent browser' });
    await expect(browserChoice).toBeChecked();
    await expect(browserChoice).toBeDisabled();
    await page.keyboard.press('Escape');
    await page.screenshot({ path: info.outputPath('home-recorded-intent.png') });
    observation.mark('firstIntent');
    await page.getByRole('button', { name: 'Recover original task' }).click();
    await expect(page.getByRole('button', { name: 'Recover original task' })).toBeVisible();
    await page.reload();
    await page.getByRole('button', { name: 'Recover original task' }).click();
    await expect(page).toHaveURL(/\/agents\/agt_/, { timeout: 90000 });
    const agentId = new URL(page.url()).pathname.split('/').at(-1)!;
    const answer = page.getByRole('article').filter({ hasText: 'The comparison is complete.' });
    const download = answer.getByRole('button', { name: 'Download report.txt', exact: true });
    await expect(download).toBeEnabled({ timeout: 90000 });
    await answer.getByRole('button', { name: 'Preview report.txt', exact: true }).click();
    await expect(page.getByRole('complementary', { name: 'Deliverable preview' })).toContainText(
      'Tool A',
    );
    await page.keyboard.press('Escape');
    const pendingDownload = page.waitForEvent('download');
    observation.mark('downloadStarted');
    await download.click();
    const downloaded = await pendingDownload;
    const bytes = await readFile((await downloaded.path())!);
    expect(bytes.toString()).toBe('Tool A: simple\nTool B: flexible\nTool C: portable\n');
    observation.mark('downloadFinished');
    if (ready.control) await composedControl(page, info, directory, agentId);
    await answer.hover();
    observation.mark('feedbackStarted');
    await answer.getByRole('button', { name: 'Needs improvement', exact: true }).click();
    const reasons = answer.getByRole('region', { name: 'Reasons for improvement' });
    await reasons.getByRole('checkbox', { name: 'Incomplete answer' }).check();
    await reasons
      .getByRole('textbox', { name: 'Additional details (optional)' })
      .fill('Please explain the flexibility trade-off.');
    await reasons.getByRole('button', { name: 'Save reasons', exact: true }).click();
    await expect(reasons.getByText('Reasons saved', { exact: true })).toBeVisible();
    observation.mark('feedbackSaved');
    await reasons.getByRole('button', { name: 'Close', exact: true }).click();
    if (process.env.CMA_INTEGRATED_VISUALS === '1') {
      const released = page.waitForResponse(
        (response) =>
          response.request().method() === 'DELETE' &&
          new URL(response.url()).pathname.endsWith('/browser'),
      );
      await page.getByRole('button', { name: 'Back to conversation', exact: true }).click();
      expect((await released).status()).toBe(200);
      await composedAccessibility(
        page.context().browser()!,
        new URL(page.url()).origin,
        agentId,
        info,
        directory,
      );
    }
    const edit = page.getByRole('button', { name: 'Edit suggestion 1 before sending' });
    await expect(edit).toBeEnabled({ timeout: 30000 });
    await edit.click();
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(composer).toBeFocused();
    const edited = 'Explain the flexibility trade-off in the comparison in two sentences.';
    await composer.fill(edited);
    observation.mark('secondSent');
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    if (process.env.CMA_INTEGRATED_EXTENDED === '1')
      await reconnectRunningTurn(page, directory, agentId, edited);
    await expect(
      page.getByText('The flexibility trade-off is explained.', { exact: true }),
    ).toBeVisible({ timeout: 90000 });
    observation.mark('secondCompleted');
    if (process.env.CMA_INTEGRATED_EXTENDED === '1') {
      const direct = page.getByRole('button', {
        name: 'Compare maintenance effort for the three tools.',
        exact: true,
      });
      await expect(direct).toBeEnabled({ timeout: 30000 });
      observation.mark('directSent');
      await direct.click();
      await expect(
        page.getByText('Maintenance effort compared for all three tools.', { exact: true }),
      ).toBeVisible({ timeout: 90000 });
      observation.mark('directCompleted');
    }
    await page.reload();
    await expect(download).toBeEnabled();
    await expect(
      answer.getByRole('button', { name: 'Needs improvement', exact: true }),
    ).toHaveAttribute('aria-pressed', 'true');
    await answer.getByRole('button', { name: 'Edit reasons', exact: true }).click();
    await expect(reasons.getByRole('checkbox', { name: 'Incomplete answer' })).toBeChecked();
    await expect(
      reasons.getByRole('textbox', { name: 'Additional details (optional)' }),
    ).toHaveValue('Please explain the flexibility trade-off.');
    await expect(
      page.getByText('The flexibility trade-off is explained.', { exact: true }),
    ).toBeVisible();
    await page.screenshot({ path: info.outputPath('completed-after-reload.png'), fullPage: true });
    await retainedAssets(page, directory, agentId);
    if (process.env.CMA_INTEGRATED_EXTENDED === '1')
      await authorityBoundaries(page, directory, agentId);
    await Promise.all(failureReads);
    expect(creationCount).toBe(2);
    await writeFile(
      join(directory, 'journey-metrics.json'),
      JSON.stringify(await observation.result(), null, 2),
    );
    await writeFile(
      join(directory, 'web-result.json'),
      JSON.stringify(
        {
          storyComplete: true,
          passed: errors.length === 0 && failures.length === 0,
          agentId,
          edited,
          creationCount,
          writes,
          errors,
          failures,
          fileSha256: createHash('sha256').update(bytes).digest('hex'),
          fileBytes: bytes.length,
        },
        null,
        2,
      ),
    );
    expect(errors).toEqual([]);
    expect(failures).toEqual([]);
  } finally {
    await Promise.all(failureReads);
    await writeFile(
      join(directory, 'journey-observations.json'),
      JSON.stringify(await observation.result(), null, 2),
    );
    await writeFile(
      join(directory, 'web-observations.json'),
      JSON.stringify({ errors, failures, writes, creationCount }, null, 2),
    );
  }
});
