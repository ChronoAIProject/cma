import { expect, test, type Page } from '@playwright/test';
import { navigation, noEffects } from './resource-lifecycle.fixture';
import { originalRequest, retainedAnswer, retiredFixture } from './retired-agent.fixture';

const notice = "This agent's cloud computer was deleted. The conversation is kept read-only.";
async function noRuntimeControls(page: Page) {
  for (const name of ['Message', 'Follow-up draft'])
    await expect(page.getByRole('textbox', { name, exact: true })).toHaveCount(0);
  await expect(page.locator('.agent-side-panel')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'More Agent actions' })).toHaveCount(0);
  await expect(page.getByText(notice, { exact: true })).toHaveCount(1);
}

for (const theme of ['dark', 'light']) {
  for (const width of [1440, 390, 320]) {
    test(`LOCAL retired ${theme} ${width} direct reload exports exact history and imports only an explicit draft`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      const local = await retiredFixture(page, baseURL!);
      await page.goto('/agent-manager');
      await page.getByLabel('Your task', { exact: true }).fill('Independent Home draft');
      await page.goto(`/agents/${local.boundary.agent.id}`);
      await expect(page.getByText(retainedAnswer, { exact: true })).toBeVisible();
      await noRuntimeControls(page);
      expect(local.reads.filter((path) => path.endsWith('/original-request'))).toEqual([]);
      await page.reload();
      await expect(page.getByText(retainedAnswer, { exact: true })).toBeVisible();
      await noRuntimeControls(page);
      await navigation(page, true);
      await expect(
        page.getByRole('group', { name: 'Retired', exact: true }).getByRole('link'),
      ).toHaveCount(1);
      await navigation(page, false);
      await expect(page.getByText(/A new message continues this Agent conversation/)).toHaveCount(
        0,
      );
      const bounds = await page.locator('.retained-agent').evaluate((host) => {
        const notice = host.querySelector('.retained-agent__notice')!.getBoundingClientRect();
        const history = host.querySelector('.agent-surface__conversation')!.getBoundingClientRect();
        return { noticeBottom: notice.bottom, historyTop: history.top };
      });
      expect(bounds.historyTop).toBeGreaterThanOrEqual(bounds.noticeBottom);
      await page.locator('.agent-surface__conversation').evaluate((element) => {
        element.scrollTop = 0;
      });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      await page.screenshot({
        path: info.outputPath(`retired-${theme}-${width}.png`),
        fullPage: true,
      });
      const downloading = page.waitForEvent('download');
      await page.getByRole('button', { name: 'Export conversation', exact: true }).click();
      const file = await downloading;
      const stream = await file.createReadStream();
      const chunks: Buffer[] = [];
      for await (const chunk of stream!) chunks.push(Buffer.from(chunk));
      const records = Buffer.concat(chunks)
        .toString('utf8')
        .trimEnd()
        .split('\n')
        .map((line) => JSON.parse(line));
      expect(records[0]).toMatchObject({
        format: 'cma.retained-conversation',
        through_seq: 4,
        coverage: { raw_history: false, completeness: 'not_certified' },
      });
      expect(records.slice(1).map((record) => record.content)).toEqual([
        originalRequest,
        retainedAnswer,
      ]);
      expect(records[1].outcome).toEqual(local.transcript.turns[0]!.outcome);
      await page
        .getByRole('button', { name: 'Start a new agent with this request', exact: true })
        .click();
      const dialog = page.getByRole('dialog', { name: 'Use this request in Home?' });
      await expect(dialog.locator('pre')).toHaveText(originalRequest);
      await dialog.getByRole('button', { name: 'Keep current draft' }).click();
      await noRuntimeControls(page);
      await page
        .getByRole('button', { name: 'Start a new agent with this request', exact: true })
        .click();
      await dialog.getByRole('button', { name: 'Use this request', exact: true }).click();
      await expect(page.getByLabel('Your task', { exact: true })).toHaveValue(originalRequest);
      await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeEnabled();
      noEffects(local.boundary);
      expect(local.runtime).toEqual([]);
    });
  }

  test(`LOCAL retired ${theme} history failure retries only its reader and unavailable original never starts work`, async ({
    page,
    baseURL,
  }, info) => {
    await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
    const local = await retiredFixture(page, baseURL!);
    local.historyError(true);
    local.originalAvailable(false);
    await page.goto(`/agents/${local.boundary.agent.id}`);
    await expect(page.getByText('Conversation couldn’t be loaded', { exact: true })).toBeVisible();
    await noRuntimeControls(page);
    await page
      .getByRole('button', { name: 'Start a new agent with this request', exact: true })
      .click();
    await expect(page.getByText('Original request unavailable', { exact: true })).toBeVisible();
    await page.screenshot({
      path: info.outputPath(`retired-read-failure-${theme}.png`),
      fullPage: true,
    });
    const readsBeforeRetry = local.reads.filter((path) => path.endsWith('/transcript')).length;
    local.historyError(false);
    await page.getByRole('button', { name: 'Load conversation again' }).click();
    await expect(page.getByText(retainedAnswer, { exact: true })).toBeVisible();
    expect(local.reads.filter((path) => path.endsWith('/transcript'))).toHaveLength(
      readsBeforeRetry + 1,
    );
    await noRuntimeControls(page);
    expect(local.runtime).toEqual([]);
    noEffects(local.boundary);
  });
}
