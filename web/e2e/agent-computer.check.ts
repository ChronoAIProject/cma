import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';

test('R pinned native command output survives the public projection and page reload', async ({
  page,
  baseURL,
}, info) => {
  const proof = process.env.CMA_PUBLIC_OUTPUT_PROOF_FILE;
  test.skip(!proof, 'Requires the isolated native ACP capture projected by the candidate core');
  const output = readFileSync(proof!, 'utf8');
  const fixture = new LocalAgentV2();
  fixture.enabled = false;
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v1/agui/events**', (route) =>
      route.fulfill({ contentType: 'text/event-stream', body: output }),
    );
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
      route.fulfill({ json: { path: '', entries: [] } }),
    );
    await page.goto('/agents/agt_contract');
    for (const attempt of ['initial', 'reload']) {
      if (attempt === 'reload') await page.reload();
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
      await computer.getByRole('tab', { name: 'Operations' }).click();
      const operation = computer
        .getByRole('region', { name: 'Turn 3', exact: true })
        .getByRole('article');
      await expect(operation.locator('pre')).toHaveText('native-computer-proof\n');
      await expect(operation.getByText('Completed', { exact: true })).toBeVisible();
      expect(
        await computer
          .getByRole('tabpanel', { name: 'Operations' })
          .locator('input, textarea, [contenteditable=true], iframe')
          .count(),
      ).toBe(0);
    }
    await page.screenshot({ path: info.outputPath('computer-native-output.png') });
    expect(fixture.writes).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL scoped environment refresh withdraws existing output and reports browser availability', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  let availability = 'not_enabled';
  let outputAvailability = 'available';
  try {
    fixture.append({ type: 'turn_started', turn_sequence: 1 });
    fixture.append({
      type: 'tool_started',
      call_id: 'scoped-command',
      title: 'printf environment-proof',
      kind: 'execute',
      status: 'in_progress',
    });
    fixture.append({
      type: 'tool_output_reported',
      call_id: 'scoped-command',
      output: { text: 'environment-proof', truncated: false },
    });
    await page.setViewportSize({ width: 1600, height: 1000 });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          active_turn: null,
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
      route.fulfill({ json: { path: '', entries: [] } }),
    );
    await page.route('**/api/v1/agents/agt_contract/environment', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_contract',
          policy_revision: 1,
          runtime_incarnation: 'b'.repeat(64),
          revalidate_at: null,
          files: {
            state: 'available',
            reason: 'authorized_mounted_home',
            scope: 'mounted_home',
            writable: true,
          },
          operation_output: {
            state: outputAvailability,
            reason: 'recorded_output_only',
            scope: 'agent_turn_history',
            writable: false,
          },
          browser: {
            state: availability,
            reason: 'browser_disabled',
            scope: 'agent_runtime',
            writable: false,
          },
          replay: {
            state: 'unsupported',
            reason: 'not_supported_by_contract',
            scope: 'none',
            writable: false,
          },
          control: {
            state: 'unsupported',
            reason: 'not_supported_by_contract',
            scope: 'none',
            writable: false,
          },
        },
      }),
    );
    await page.goto('/agents/agt_contract');
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await computer.getByRole('tab', { name: 'Operations' }).click();
    await expect(computer.locator('pre')).toHaveText('environment-proof');
    await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
    await expect(computer.getByText('Live screen is not enabled', { exact: true })).toBeVisible();
    availability = 'forbidden';
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect(
      computer.getByText('Live screen access is not permitted.', { exact: true }),
    ).toBeVisible();
    expect(await computer.locator('iframe, video, canvas').count()).toBe(0);
    await computer.getByRole('tab', { name: 'Operations' }).click();
    await expect(computer.locator('pre')).toHaveText('environment-proof');
    outputAvailability = 'forbidden';
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect(computer.getByText('Operation output access is not permitted.')).toBeVisible();
    await expect(computer.locator('pre')).toHaveCount(0);
    fixture.append({
      type: 'tool_output_reported',
      call_id: 'scoped-command',
      output: { text: 'late environment-proof', truncated: false },
    });
    await expect(computer.locator('pre')).toHaveCount(0);
    expect(fixture.writes).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

for (const width of [390, 1600]) {
  test(`LOCAL computer starts closed at ${width}px and preserves the composer through close and reopen`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    try {
      await page.setViewportSize({ width, height: 844 });
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(0),
            active_turn: null,
          },
        }),
      );
      await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
        route.fulfill({ json: { path: '', entries: [] } }),
      );
      await page.goto('/agents/agt_contract');
      const composer = page.getByRole('textbox', { name: 'Message', exact: true });
      await composer.fill('Retained draft');
      const original = await composer.elementHandle();
      const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
      await expect(computer).toBeHidden();
      await page.setViewportSize({ width: width === 390 ? 1600 : 390, height: 844 });
      await expect(computer).toBeHidden();
      await page.setViewportSize({ width, height: 844 });
      await expect(computer).toBeHidden();
      const entry = page.getByRole('button', { name: 'Show Agent computer', exact: true });
      await entry.click();
      await expect(computer).toBeVisible();
      await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
      await expect(
        computer.getByText('Live screen is not supported by this environment.', { exact: true }),
      ).toBeVisible();
      await page.screenshot({ path: info.outputPath('computer-mobile.png') });
      await computer.getByRole('button', { name: 'Close computer', exact: true }).click();
      await expect(computer).toBeHidden();
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      await page.setViewportSize({ width: width === 390 ? 1600 : 390, height: 844 });
      await expect(computer).toBeHidden();
      await entry.click();
      await expect(computer).toBeVisible();
      await expect(composer).toHaveValue('Retained draft');
      expect(await composer.evaluate((element, previous) => element === previous, original)).toBe(
        true,
      );
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

test('LOCAL files and recorded turn output preserve the composer and reading position', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalAgentV2();
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.append({
      type: 'tool_started',
      call_id: 'proof',
      title: 'printf computer-proof',
      kind: 'execute',
      status: 'in_progress',
    });
    fixture.append({
      type: 'tool_updated',
      call_id: 'proof',
      status: 'completed',
      title: null,
      output: { text: 'computer-proof', truncated: false },
    });
    fixture.append({
      type: 'message',
      role: 'assistant',
      content: 'Recorded answer.\n\n'.repeat(100),
    });
    fixture.append({ type: 'turn_completed', turn_sequence: 1 });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          active_turn: null,
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
      route.fulfill({
        json: {
          path: new URL(route.request().url()).searchParams.get('path') ?? '',
          entries: [
            {
              path: 'cma/project/.worktrees/agt_contract/notes.txt',
              name: 'notes.txt',
              kind: 'file',
              size_bytes: 5,
              hidden: false,
            },
          ],
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/files/content**', (route) =>
      route.fulfill({
        json: {
          path: 'cma/project/.worktrees/agt_contract/notes.txt',
          content_base64: Buffer.from('Notes').toString('base64'),
          size_bytes: 5,
          sha256: 'a'.repeat(64),
        },
      }),
    );
    await page.goto('/agents/agt_contract');
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(composer).toBeEnabled();
    await composer.fill('Keep my unsent draft');
    const original = await composer.elementHandle();
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await expect(computer).toBeHidden();
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    await expect(computer).toBeVisible();
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    await computer.getByRole('button', { name: 'notes.txt', exact: true }).click();
    await computer
      .getByRole('textbox', { name: 'Contents of notes.txt' })
      .fill('Keep my file draft');
    const scroll = page.locator('.agent-surface__conversation');
    await scroll.evaluate((element) => {
      element.scrollTop = 100;
    });
    const position = await scroll.evaluate((element) => element.scrollTop);
    await computer.getByRole('tab', { name: 'Operations' }).click();
    await expect(computer.getByText('computer-proof', { exact: true })).toBeVisible();
    await expect(
      computer.getByRole('article', { name: 'Operation: printf computer-proof', exact: true }),
    ).toBeVisible();
    await expect(composer).toHaveValue('Keep my unsent draft');
    expect(await composer.evaluate((element, previous) => element === previous, original)).toBe(
      true,
    );
    expect(await scroll.evaluate((element) => element.scrollTop)).toBe(position);
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    await expect(
      computer.getByRole('textbox', {
        name: 'Contents of notes.txt',
      }),
    ).toHaveValue('Keep my file draft');
    await page.screenshot({ path: info.outputPath('computer-desktop.png') });
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
  } catch (error) {
    console.log({
      body: await page.locator('body').innerText(),
      errors: fixture.errors,
      unexpected: fixture.boundary.unexpected,
    });
    throw error;
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL capability checks recover Files, Screen and Operations after 503 without writes', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalAgentV2();
  let reads = 0;
  let activeReads = 0;
  let peakReads = 0;
  let fileReads = 0;
  let release!: () => void;
  let releaseRecovery!: () => void;
  const pending = new Promise<void>((resolve) => (release = resolve));
  const pendingRecovery = new Promise<void>((resolve) => (releaseRecovery = resolve));
  const unsupported = {
    state: 'unsupported',
    reason: 'not_supported_by_contract',
    scope: 'none',
    writable: false,
  };
  const capabilities = {
    agent_id: 'agt_contract',
    policy_revision: 1,
    runtime_incarnation: 'b'.repeat(64),
    revalidate_at: null,
    files: {
      state: 'available',
      reason: 'authorized_mounted_home',
      scope: 'mounted_home',
      writable: true,
    },
    operation_output: {
      state: 'available',
      reason: 'recorded_output_only',
      scope: 'agent_turn_history',
      writable: false,
    },
    browser: { ...unsupported, state: 'not_enabled', reason: 'browser_disabled' },
    replay: unsupported,
    control: unsupported,
  };
  try {
    fixture.append({ type: 'turn_started', turn_sequence: 1 });
    fixture.append({
      type: 'tool_started',
      call_id: 'capability',
      title: 'printf capability-proof',
      kind: 'execute',
      status: 'in_progress',
    });
    fixture.append({
      type: 'tool_updated',
      call_id: 'capability',
      status: 'completed',
      title: null,
      output: { text: 'capability-proof', truncated: false },
    });
    fixture.append({ type: 'turn_completed', turn_sequence: 1 });
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.clock.install();
    await page.clock.pauseAt(new Date(Date.now() + 1000));
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          active_turn: null,
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/environment', async (route) => {
      expect(route.request().method()).toBe('GET');
      const attempt = ++reads;
      activeReads++;
      peakReads = Math.max(peakReads, activeReads);
      if (attempt === 1) await pending;
      if (attempt === 2) await pendingRecovery;
      await route.fulfill(
        attempt > 1
          ? { json: capabilities }
          : {
              status: 503,
              headers: { 'content-type': 'application/problem+json', 'retry-after': '1' },
              json: {
                type: 'about:blank',
                title: 'Agent owner unavailable',
                status: 503,
                code: 'agent_owner_unavailable',
              },
            },
      );
      activeReads--;
    });
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) => {
      expect(route.request().method()).toBe('GET');
      fileReads++;
      return route.fulfill({
        json: { path: new URL(route.request().url()).searchParams.get('path') ?? '', entries: [] },
      });
    });
    await page.goto('/agents/agt_contract');
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    for (const name of ['Files', 'Operations']) {
      await computer.getByRole('tab', { name, exact: true }).click();
      await expect(computer.getByRole('tabpanel', { name, exact: true })).toContainText(
        name === 'Files' ? 'Checking file access…' : 'Checking operation availability…',
      );
    }
    expect(fileReads).toBe(0);
    release();
    for (const name of ['Operations', 'Screen', 'Files']) {
      await computer.getByRole('tab', { name, exact: true }).click();
      const tab = computer.getByRole('tabpanel', { name, exact: true });
      const subject = name === 'Files' ? 'file' : name === 'Screen' ? 'screen' : 'operation';
      await expect(tab).toContainText(`We couldn’t check ${subject} availability.`);
      await expect(tab.getByRole('status')).toHaveCount(1);
      await expect(tab.getByRole('button', { name: 'Try again' })).toBeVisible();
    }
    expect(fileReads).toBe(0);
    const retry = computer.getByRole('button', { name: 'Try again', exact: true });
    await retry.click({ clickCount: 2 });
    await page.clock.runFor(999);
    expect(reads).toBe(1);
    await page.clock.runFor(300);
    await expect.poll(() => reads).toBe(2);
    await retry.click({ clickCount: 2 });
    releaseRecovery();
    await expect.poll(() => fileReads).toBe(1);
    expect(reads).toBe(2);
    const assertRecovered = async () => {
      await computer.getByRole('tab', { name: 'Files', exact: true }).click();
      await expect(
        computer.getByRole('tabpanel', { name: 'Files', exact: true }),
      ).not.toContainText('unavailable');
      await expect(computer.getByRole('button', { name: 'New file', exact: true })).toBeVisible();
      await computer.getByRole('tab', { name: 'Operations', exact: true }).click();
      await expect(computer.getByText('capability-proof', { exact: true })).toBeVisible();
      await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
      await expect(computer.getByText('Live screen is not enabled', { exact: true })).toBeVisible();
      await expect(computer.getByRole('button', { name: 'Try again' })).toHaveCount(0);
      await expect(computer.locator('iframe, video, canvas')).toHaveCount(0);
    };
    await assertRecovered();
    await computer.getByRole('button', { name: 'Close computer', exact: true }).click();
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    await assertRecovered();
    await page.reload();
    await expect(computer).toBeHidden();
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    await assertRecovered();
    await page.screenshot({ path: info.outputPath('capability-recovery.png') });
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.writes).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
    expect(fixture.errors).toEqual([]);
    expect(peakReads).toBe(1);
  } finally {
    release();
    releaseRecovery();
    await fixture.close(page);
  }
});
