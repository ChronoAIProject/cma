import { test, expect } from '@playwright/test';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';

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
