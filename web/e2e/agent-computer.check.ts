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
      const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
      await computer.getByRole('tab', { name: 'Operation output' }).click();
      const operation = computer
        .getByRole('region', { name: 'Turn 3', exact: true })
        .getByRole('article');
      await expect(operation.locator('pre')).toHaveText('native-computer-proof\n');
      await expect(operation.getByText('Completed', { exact: true })).toBeVisible();
      expect(
        await computer
          .getByRole('tabpanel', { name: 'Operation output' })
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

test('LOCAL browser availability comes from the scoped public environment observation', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  let availability = 'not_enabled';
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
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
            state: 'available',
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
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await computer.getByRole('tab', { name: 'Browser', exact: true }).click();
    await expect(
      computer.getByText('Browser is not enabled for this environment.', { exact: true }),
    ).toBeVisible();
    availability = 'forbidden';
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await expect(
      computer.getByText('Browser access is not permitted.', { exact: true }),
    ).toBeVisible();
    expect(await computer.locator('iframe, video, canvas').count()).toBe(0);
    expect(fixture.writes).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL ten thousand operation turns stay paged while assistant text streams', async ({
  page,
  baseURL,
}, info) => {
  test.setTimeout(120_000);
  const fixture = new LocalAgentV2();
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    for (let turn = 1; turn <= 10_000; turn += 1) {
      const sequence = fixture.items.length + 1;
      fixture.append({ type: 'turn_started', turn_sequence: sequence });
      fixture.append({
        type: 'tool_started',
        call_id: `proof-${turn}`,
        title: `printf proof-${turn}`,
        kind: 'execute',
        status: 'in_progress',
      });
      fixture.append({
        type: 'tool_updated',
        call_id: `proof-${turn}`,
        status: 'completed',
        title: null,
        output: { text: `proof-${turn}`, truncated: false },
      });
      fixture.append({ type: 'turn_completed', turn_sequence: sequence });
    }
    fixture.append({ type: 'turn_started', turn_sequence: 40_001 });
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
    await page.route('**/api/v2/agents/agt_contract/events/snapshot**', (route) => {
      const url = new URL(route.request().url());
      const offset = Number(url.searchParams.get('page_cursor')?.split('.').at(-1) ?? 0);
      const limit = Number(url.searchParams.get('limit') ?? 100);
      const end = Math.min(fixture.items.length, offset + limit);
      return route.fulfill({
        json: {
          object: 'cma.agent.event_page',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          items: fixture.items.slice(offset, end),
          next_page_cursor:
            end < fixture.items.length
              ? `ags1.${'a'.repeat(64)}.${fixture.items.length}.${end}`
              : null,
        },
      });
    });
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
      route.fulfill({ json: { path: '', entries: [] } }),
    );
    await page.goto('/agents/agt_contract');
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await computer.getByRole('tab', { name: 'Operation output' }).click();
    await expect(
      computer.getByRole('article', { name: 'Operation: printf proof-10000', exact: true }),
    ).toBeVisible({ timeout: 90_000 });
    expect(await computer.locator('article').count()).toBeLessThanOrEqual(50);
    await computer.getByRole('button', { name: 'Older operations' }).click();
    await expect(
      computer.getByRole('article', { name: 'Operation: printf proof-9950', exact: true }),
    ).toBeVisible();
    await computer.getByRole('button', { name: 'Newer operations' }).click();
    const output = await computer.getByText('proof-10000', { exact: true }).elementHandle();
    await page.evaluate(() => {
      const output = document.querySelector('#computer-page-activity')!;
      const observed = {
        changes: 0,
        observer: new MutationObserver((changes) => {
          observed.changes += changes.length;
        }),
      };
      observed.observer.observe(output, { subtree: true, childList: true, characterData: true });
      Object.assign(window, { computerOutputObservation: observed });
    });
    for (let chunk = 0; chunk < 100; chunk += 1) {
      for (const stream of fixture.streams)
        stream.write(
          `event: cma.agent.live\ndata: ${JSON.stringify({ agent_id: 'agt_contract', incarnation: 'a'.repeat(64), turn_sequence: 40_001, after_sequence: fixture.items.length, message_id: 'proof-stream', kind: 'text_delta', delta: 'stream-proof ' })}\n\n`,
        );
    }
    await expect(page.locator('.agent-surface__conversation')).toContainText('stream-proof');
    expect(
      await computer
        .getByText('proof-10000', { exact: true })
        .evaluate((element, previous) => element === previous, output),
    ).toBe(true);
    expect(
      await page.evaluate(
        () =>
          (window as unknown as { computerOutputObservation: { changes: number } })
            .computerOutputObservation.changes,
      ),
    ).toBe(0);
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    expect(await page.locator('#computer-page-activity article').count()).toBe(0);
    await page.screenshot({ path: info.outputPath('computer-10000-turns.png') });
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL narrow computer entry returns keyboard focus and preserves the composer', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalAgentV2();
  try {
    await page.setViewportSize({ width: 390, height: 844 });
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
    await composer.fill('Mobile draft');
    const original = await composer.elementHandle();
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await expect(computer).toBeHidden();
    const entry = page.getByRole('button', { name: 'Show Agent computer', exact: true });
    await entry.click();
    await expect(computer).toBeVisible();
    await expect(computer.getByRole('button', { name: 'Back to conversation' })).toBeFocused();
    await computer.getByRole('tab', { name: 'Browser', exact: true }).click();
    await expect(
      computer.getByText('Environment capabilities are unavailable. Try again.', { exact: true }),
    ).toBeVisible();
    await page.screenshot({ path: info.outputPath('computer-mobile.png') });
    await page.keyboard.press('Escape');
    await expect(computer).toBeHidden();
    await expect(entry).toBeFocused();
    await expect(composer).toHaveValue('Mobile draft');
    expect(await composer.evaluate((element, previous) => element === previous, original)).toBe(
      true,
    );
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

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
          path: '',
          entries: [
            { path: 'notes.txt', name: 'notes.txt', kind: 'file', size_bytes: 5, hidden: false },
          ],
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/files/content**', (route) =>
      route.fulfill({
        json: {
          path: 'notes.txt',
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
    await computer.getByRole('tab', { name: 'Operation output' }).click();
    await expect(computer.getByText('computer-proof', { exact: true })).toBeVisible();
    await expect(computer.getByRole('heading', { name: 'Turn 1', exact: true })).toBeVisible();
    await expect(composer).toHaveValue('Keep my unsent draft');
    expect(await composer.evaluate((element, previous) => element === previous, original)).toBe(
      true,
    );
    expect(await scroll.evaluate((element) => element.scrollTop)).toBe(position);
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    await expect(computer.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveValue(
      'Keep my file draft',
    );
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
