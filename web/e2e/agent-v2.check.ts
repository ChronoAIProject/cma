import './agent-v2-tool-timing.cases';
import { test, expect, type Page } from '@playwright/test';
import { binding, LocalAgentV2 } from './agent-v2.fixture';
import { traceOpeningReads, openingReads } from './agent-v2-reads.fixture';

const message = (page: Page) => page.getByRole('textbox', { name: 'Message', exact: true });
const thinking = (page: Page) => page.getByRole('status').filter({ hasText: /^Running a tool$/ });
const recover = (page: Page) => page.getByRole('button', { name: 'Recover pending request' });
function clean(fixture: LocalAgentV2) {
  expect(fixture.boundary.writes).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.errors).toEqual([]);
}
async function open(page: Page, fixture: LocalAgentV2, baseURL: string) {
  await fixture.route(page, baseURL);
  await page.goto('/agents/agt_contract');
  await expect(message(page)).toBeVisible();
}

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: independent input receipt and rich replay preserve accessible chronology`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.holdInput = true;
    try {
      await open(page, fixture, baseURL!);
      await expect.poll(() => fixture.streams.size).toBe(1);
      await page.evaluate(() => {
        document.body.dataset.recoveryPanelSeen = 'false';
        const observe = (node: Node) => {
          if (
            node instanceof Element &&
            (node.matches('.agent-pending-panel') || node.querySelector('.agent-pending-panel'))
          )
            document.body.dataset.recoveryPanelSeen = 'true';
        };
        new MutationObserver((records) => {
          for (const record of records) for (const node of record.addedNodes) observe(node);
        }).observe(document.body, { childList: true, subtree: true });
      });
      await message(page).fill('Inspect the repository.');
      await page.getByRole('button', { name: 'Send message', exact: true }).click();
      await expect.poll(() => fixture.writes.length).toBe(1);
      fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      fixture.append({ type: 'message', role: 'user', content: 'Inspect the repository.' });
      fixture.append({
        type: 'tool_started',
        call_id: 'slow',
        title: 'Run tests',
        kind: 'execute',
        status: 'running',
      });
      fixture.append({
        type: 'tool_started',
        call_id: 'fast',
        title: 'Read package',
        kind: 'read',
        status: 'running',
      });
      fixture.live('reasoning_summary_delta', 'Inspecting dependencies');
      const toggle = page.getByRole('button', { name: 'View activity', exact: true });
      await expect(thinking(page)).toBeVisible();
      await expect(toggle).toBeVisible();
      await expect(page.locator('.thinking__elapsed')).toHaveText(/^2m \d+s$/);
      await toggle.focus();
      await page.keyboard.press('Enter');
      await expect(toggle).toBeFocused();
      await expect(toggle).toHaveAttribute('aria-expanded', 'true');
      const body = await toggle.getAttribute('aria-controls');
      expect(body).toBeTruthy();
      expect(await page.evaluate((id) => document.getElementById(id!) !== null, body)).toBe(true);
      await expect(page.locator('.thinking__reasoning')).toHaveText('Inspecting dependencies');
      await expect(page.locator('.thinking__activity')).toHaveCount(2);
      await expect(page.locator('.agent-event--tool')).toHaveCount(0);
      await expect(message(page)).toHaveValue('');
      await expect(page.locator('.message--user').filter({ hasText: 'Sending…' })).toContainText(
        'Inspect the repository.',
      );
      await expect(recover(page)).toHaveCount(0);
      await expect(page.getByLabel('Pending request')).toHaveCount(0);
      expect(fixture.streams.size).toBe(1);
      fixture.acceptInput();
      await expect(message(page)).toHaveValue('');
      await expect(recover(page)).toHaveCount(0);
      await expect(page.locator('body')).toHaveAttribute('data-recovery-panel-seen', 'false');
      const text = 'Live progress remains readable.\n\n'.repeat(70);
      fixture.live('text_delta', text);
      await expect(page.locator('.message--assistant')).toContainText(
        'Live progress remains readable.',
      );
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      const activityPanel = page.locator('.thinking');
      const fits = () =>
        page.evaluate(() => {
          const dock = document.querySelector('.thinking')!.getBoundingClientRect();
          const history = document
            .querySelector('.agent-surface__conversation')!
            .getBoundingClientRect();
          const composer = document.querySelector('.agent-composer')!.getBoundingClientRect();
          return (
            document
              .querySelector('.message-stream')!
              .contains(document.querySelector('.thinking')) &&
            dock.top >= history.top - 1 &&
            dock.bottom <= history.bottom + 1 &&
            history.bottom <= composer.top + 1 &&
            dock.left >= 0 &&
            dock.right <= innerWidth &&
            document.documentElement.scrollWidth <= innerWidth + 1
          );
        });
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await activityPanel.scrollIntoViewIfNeeded();
        await expect.poll(fits).toBe(true);
        await page.locator('.agent-surface__conversation').evaluate((element) => {
          element.scrollTop = 0;
          element.dispatchEvent(new Event('scroll'));
        });
        await expect
          .poll(() =>
            page.locator('.thinking').evaluate((element) => {
              const history = element.closest('.agent-surface__conversation');
              return (
                !!history &&
                element.getBoundingClientRect().top >= history.getBoundingClientRect().bottom
              );
            }),
          )
          .toBe(true);
        await activityPanel.scrollIntoViewIfNeeded();
        await expect.poll(fits).toBe(true);
        await page.screenshot({
          path: info.outputPath(`agent-v2-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await expect.poll(fits).toBe(true);
      await page.setViewportSize({ width: 1440, height: 900 });
      fixture.append({ type: 'tool_updated', call_id: 'fast', status: 'completed', title: null });
      await expect(page.locator('.thinking__activity')).toHaveCount(1);
      await page.locator('.tool-calls__toggle[aria-expanded="false"]').click();
      await expect(page.locator('.tool-call__literal')).toHaveText(['Read package']);
      fixture.append({ type: 'tool_updated', call_id: 'slow', status: 'failed', title: null });
      await expect(page.locator('.thinking__activity')).toHaveCount(0);
      await expect(page.locator('.tool-call__literal')).toHaveText(['Read package', 'Run tests']);
      fixture.append({ type: 'message', role: 'assistant', content: text });
      await page.reload();
      await expect(toggle).toBeVisible();
      await expect(page.locator('.thinking__reasoning')).toHaveCount(0);
      await page.locator('.tool-calls__toggle[aria-expanded="false"]').click();
      await expect(page.locator('.tool-call__literal')).toHaveText(['Read package', 'Run tests']);
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.append({ type: 'turn_completed', turn_sequence: 1 });
      await expect(toggle).toHaveCount(0);
      await expect(page.locator('.agent-event--tool')).toHaveCount(2);
      expect(fixture.writes).toHaveLength(1);
      clean(fixture);
    } finally {
      await fixture.close(page);
    }
  });
}

test('LOCAL: explicit input recovery after reload keeps its body, key and selected transport', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.loseInput = true;
  fixture.revealInputReceipt = false;
  try {
    await open(page, fixture, baseURL!);
    await message(page).fill('First line\nSecond line 🛰️');
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect(recover(page)).toBeEnabled();
    await expect(page.getByLabel('Pending request').getByRole('alert')).toContainText(
      'We can’t confirm this message arrived',
    );
    expect(fixture.writes).toHaveLength(1);
    fixture.enabled = false;
    await page.reload();
    await expect(recover(page)).toBeEnabled();
    expect(fixture.writes).toHaveLength(1);
    fixture.revealInputReceipt = true;
    await recover(page).click();
    await expect(recover(page)).toHaveCount(0);
    expect(fixture.writes).toHaveLength(1);
    expect(fixture.writes[0]!.key).toBeTruthy();
    expect(fixture.receiptReads.length).toBeGreaterThanOrEqual(2);
    for (const lookup of fixture.receiptReads) {
      expect(lookup).toEqual({ ...fixture.writes[0], path: `${fixture.writes[0]!.path}/receipt` });
    }
    expect(fixture.writes[0]!.body).toEqual({ input: 'First line\nSecond line 🛰️', stream: true });
    await expect.poll(() => fixture.streams.size).toBe(1);
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL: targeted control recovery retains its target and delivery leaves the turn active', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
  fixture.loseControl = true;
  try {
    await open(page, fixture, baseURL!);
    await page.getByRole('button', { name: 'Stop response', exact: true }).click();
    await expect(recover(page)).toBeEnabled();
    expect(fixture.writes).toHaveLength(1);
    await page.reload();
    await expect(recover(page)).toBeEnabled();
    fixture.controlConflict = true;
    await recover(page).click();
    await expect(
      page.getByRole('alert').filter({ hasText: 'Fresh observation changed' }),
    ).toBeVisible();
    await expect(recover(page)).toBeEnabled();
    expect(fixture.writes).toHaveLength(2);
    expect(fixture.writes[1]).toEqual(fixture.writes[0]);
    fixture.controlConflict = false;
    await recover(page).click();
    await expect(recover(page)).toHaveCount(0);
    await expect.poll(() => fixture.reads).toContain('/api/v2/commands/cmd_control');
    await expect(
      page.getByRole('status').filter({ hasText: 'Your response action was delivered.' }),
    ).toHaveCount(0);
    await expect(
      page.getByRole('status').filter({ hasText: /^Waiting for a response$/ }),
    ).toBeVisible();
    expect(fixture.writes).toHaveLength(3);
    expect(fixture.writes[2]).toEqual(fixture.writes[0]);
    expect(fixture.writes[0]!.path).toBe('/api/v2/agents/agt_contract/responses/resp_1/controls');
    expect(fixture.writes[0]!.body).toEqual({
      turn_sequence: 1,
      expected_agent_revision: 10,
      command: { type: 'cancel' },
    });
    expect(fixture.reads).toContain('/api/v2/commands/cmd_control');
    await expect.poll(() => fixture.streams.size).toBe(1);
    fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
    await expect(
      page.getByRole('region', { name: 'What the agent is working through' }),
    ).toHaveCount(0);
    expect(fixture.writes).toHaveLength(3);
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL: incomplete snapshots permit input while the tail waits for the final page', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.append({ type: 'message', role: 'user', content: 'Recorded request' });
  fixture.append({ type: 'message', role: 'assistant', content: 'Recorded answer' });
  fixture.holdSnapshot = true;
  try {
    await open(page, fixture, baseURL!);
    await expect.poll(() => fixture.snapshots).toBe(2);
    await message(page).fill('Next request');
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
    expect(fixture.streams.size).toBe(0);
    await message(page).press('Enter');
    await expect.poll(() => fixture.writes.length).toBe(1);
    await expect(message(page)).toHaveValue('');
    fixture.releaseSnapshot();
    await expect(page.locator('.message--assistant')).toContainText('Recorded answer');
    await expect.poll(() => fixture.streams.size).toBe(1);
    expect(fixture.writes).toHaveLength(1);
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL: newest transcript and sending stay available during history loading and cached return', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalAgentV2();
  const previousReply = 'The newest recorded answer is visible before history finishes.';
  const nextInput = 'Continue while older activity is loading.';
  const nextReply = 'The new response remains in the cached conversation.';
  const oldBinding = { response_id: 'resp_old', command_id: 'cmd_old', turn_sequence: 1 };
  fixture.append({ type: 'turn_started', turn_sequence: 1 }, oldBinding);
  fixture.append({ type: 'message', role: 'user', content: 'Previous request' });
  fixture.append({ type: 'message', role: 'assistant', content: previousReply });
  fixture.append({ type: 'turn_completed', turn_sequence: 1 });
  fixture.holdSnapshot = true;
  let releaseUsage!: () => void;
  let releaseReturnTranscript!: () => void;
  const usageGate = new Promise<void>((resolve) => (releaseUsage = resolve));
  const returnGate = new Promise<void>((resolve) => (releaseReturnTranscript = resolve));
  let usageReads = 0;
  let returning = false;
  const transcript = {
    agent_id: 'agt_contract',
    through_seq: 4,
    turns: [
      {
        turn_sequence: 1,
        started_at: fixture.started,
        owner_message: 'Previous request',
        agent_messages: [previousReply],
        messages: [
          { sequence: 2, role: 'user', content: 'Previous request', appended_at: fixture.started },
          { sequence: 3, role: 'assistant', content: previousReply, appended_at: fixture.started },
        ],
        outcome: { type: 'completed', at: fixture.started },
      },
    ],
  };
  try {
    await traceOpeningReads(page);
    await page.clock.install();
    await fixture.route(page, baseURL!);
    await page.route('**/api/v1/agents/agt_contract/transcript', async (route) => {
      if (returning) await returnGate;
      await route.fulfill({ json: transcript });
    });
    await page.route('**/api/v1/agents/agt_contract/usage', async (route) => {
      usageReads += 1;
      await usageGate;
      await route.fulfill({
        json: {
          agent_id: 'agt_contract',
          through_seq: 4,
          input_tokens: 12,
          output_tokens: 24,
          context_used: 36,
        },
      });
    });
    await page.goto('/agents/agt_contract');
    await expect(page.getByText(previousReply, { exact: true })).toBeVisible();
    await expect.poll(() => usageReads).toBe(1);
    await expect.poll(() => fixture.snapshots).toBe(1);
    await expect(page.locator('.active-thinking')).toHaveCount(0);
    await message(page).fill(nextInput);
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect.poll(() => fixture.writes.length).toBe(1);
    expect(fixture.writes[0]!.body).toEqual({ input: nextInput, stream: true });
    await expect(message(page)).toHaveValue('');
    await expect(page.locator('.message--user').filter({ hasText: nextInput })).toContainText(
      'Sent',
    );
    await message(page).fill('Keep this unsent follow-up.');
    releaseUsage();
    await expect.poll(() => fixture.snapshots).toBe(2);
    fixture.releaseSnapshot();
    await expect.poll(() => fixture.streams.size).toBe(1);
    await expect(message(page)).toHaveValue('Keep this unsent follow-up.');
    await expect(page.locator('.message--user').filter({ hasText: nextInput })).toHaveCount(1);
    fixture.append({ type: 'turn_started', turn_sequence: 5 }, { ...binding, turn_sequence: 5 });
    fixture.append({ type: 'message', role: 'user', content: nextInput });
    fixture.append({ type: 'message', role: 'assistant', content: nextReply });
    fixture.append({ type: 'turn_completed', turn_sequence: 5 });
    await expect(page.getByText(nextReply, { exact: true })).toBeVisible();
    await expect(page.locator('.message--user').filter({ hasText: nextInput })).toHaveCount(1);
    await expect(page.locator('.message__delivery')).toHaveCount(0);
    await expect(message(page)).toHaveValue('Keep this unsent follow-up.');
    await page.screenshot({
      path: info.outputPath('nonblocking-history.png'),
      animations: 'disabled',
    });
    const completedReads = (await openingReads(page)).length;
    await page.clock.pauseAt(new Date(Date.now() + 1000));
    returning = true;
    await page.getByRole('link', { name: 'CMA home', exact: true }).click();
    await expect(page).toHaveURL(new URL('/agent-manager', baseURL!).href);
    await page.goBack();
    await expect(page).toHaveURL(/\/agents\/agt_contract$/);
    // One authorized opening batch may prepare a transcript and first page.
    // Cached rendering must not wait for it or restart paginated replay.
    await expect(page.getByText(nextReply, { exact: true })).toBeVisible();
    await expect.poll(async () => (await openingReads(page)).slice(completedReads).length).toBe(2);
    const returnReads = () => openingReads(page).then((reads) => reads.slice(completedReads));
    expect(await returnReads()).toEqual(
      expect.arrayContaining([
        { kind: 'transcript', page: 'first', phases: ['started'] },
        { kind: 'snapshot', page: 'first', phases: expect.arrayContaining(['started']) },
      ]),
    );
    await page.clock.fastForward(5001);
    await expect
      .poll(async () =>
        (await returnReads())
          .find((read) => read.kind === 'transcript')
          ?.phases.includes('aborted'),
      )
      .toBe(true);
    const trace = await returnReads();
    expect(trace).toHaveLength(2);
    expect(trace.filter((read) => read.page === 'next')).toHaveLength(0);
    expect(trace.find((read) => read.kind === 'transcript')?.phases).not.toContain('consumed');
    await info.attach('cached-return-read-lifecycle', {
      body: JSON.stringify(trace),
      contentType: 'application/json',
    });
    await expect(page.locator('.message--user').filter({ hasText: nextInput })).toHaveCount(1);
    await expect(message(page)).toHaveValue('Keep this unsent follow-up.');
    await expect(page.locator('.active-thinking')).toHaveCount(0);
    expect(fixture.writes).toHaveLength(1);
    clean(fixture);
  } finally {
    releaseUsage();
    releaseReturnTranscript();
    await fixture.close(page);
  }
});

test('LOCAL: flag reversal preserves an existing AG-UI selection', async ({ page, baseURL }) => {
  const fixture = new LocalAgentV2();
  fixture.enabled = false;
  try {
    await open(page, fixture, baseURL!);
    expect(fixture.snapshots).toBe(0);
    fixture.enabled = true;
    await page.reload();
    await expect(message(page)).toBeVisible();
    expect(fixture.snapshots).toBe(0);
    expect(fixture.streams.size).toBe(0);
    const selected = await page.evaluate(() =>
      Object.keys(sessionStorage)
        .filter((key) => key.endsWith(':local-owner:agt_contract'))
        .map((key) => JSON.parse(sessionStorage.getItem(key)!).transport),
    );
    expect(selected).toEqual(['agui']);
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL: reconnect account revalidation clears the previous account conversation', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  fixture.append({ type: 'message', role: 'assistant', content: 'Previous account conversation' });
  try {
    await open(page, fixture, baseURL!);
    await expect(page.locator('.message--assistant')).toContainText(
      'Previous account conversation',
    );
    await expect.poll(() => fixture.streams.size).toBe(1);
    fixture.owner = 'another-owner';
    fixture.endTails();
    await expect(
      page.getByText('Your sign-in changed. Open the Agent again to continue.'),
    ).toBeVisible();
    await expect(page.locator('.message--assistant')).toHaveCount(0);
    await expect(message(page)).toHaveCount(0);
    expect(fixture.reads.filter((path) => path.includes('/events?'))).toHaveLength(1);
    clean(fixture);
  } finally {
    await fixture.close(page);
  }
});
