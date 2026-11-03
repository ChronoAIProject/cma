import { expect, test, type Page } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

const text =
  'What is a simple, reliable backup strategy for SQLite in an offline desktop notes app?';
const saved = {
  status: 'ready',
  set: {
    id: 'saved-comparison',
    viewerId: 'local-owner',
    agentId: 'agt_contract',
    answerId: 'event:3',
    scope: { turn_sequence: 1, answer_sequence: 3 },
    source: { kind: 'generated', version: 'contextual-v3' },
    items: [{ id: 'saved-comparison:0', text, rank: 0 }],
  },
  observation: null,
};

async function open(page: Page, baseURL: string) {
  const fixture = new LocalAgentV2();
  page.on('pageerror', (error) => fixture.errors.push(String(error)));
  fixture.append({ type: 'turn_started', turn_sequence: 1 });
  fixture.append({
    type: 'message',
    role: 'user',
    content:
      'Compare SQLite and PostgreSQL for an offline desktop notes app used by one person. I want minimal maintenance.',
  });
  fixture.append({
    type: 'message',
    role: 'assistant',
    content:
      'SQLite fits this offline single-user app: it needs no server and stores data locally.',
  });
  fixture.append({ type: 'turn_completed', turn_sequence: 1 });
  await fixture.route(page, baseURL);
  await page.route('**/api/v1/agents/agt_contract/answers/*/deliverables', (route) =>
    route.fulfill({
      json: { agent_id: 'agt_contract', answer_id: 'event:3', turn_sequence: 1, items: [] },
    }),
  );
  await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
    route.fulfill({
      json: {
        object: 'cma.agent.event_head',
        schema_version: 1,
        agent_id: 'agt_contract',
        incarnation: 'a'.repeat(64),
        through_cursor: `ag1.${'a'.repeat(64)}.${fixture.items.length}`,
        active_turn: null,
      },
    }),
  );
  return fixture;
}

for (const width of [1440, 320]) {
  for (const theme of ['light', 'dark']) {
    test(`saved contextual result ${width}px ${theme}`, async ({ page, baseURL }, info) => {
      const fixture = await open(page, baseURL!);
      let reads = 0;
      await page.route('**/api/v1/agents/agt_contract/suggestions?*', (route) => {
        expect(route.request().method()).toBe('GET');
        expect(new URL(route.request().url()).searchParams.get('answer_sequence')).toBe('3');
        reads += 1;
        return route.fulfill({ json: reads === 1 ? { status: 'pending', set: null } : saved });
      });
      try {
        await page.setViewportSize({ width, height: 900 });
        await page.goto('/agents/agt_contract');
        const field = page.getByRole('textbox', { name: 'Message', exact: true });
        await expect(field).toBeEnabled();
        await expect(
          page.getByText(
            'SQLite fits this offline single-user app: it needs no server and stores data locally.',
          ),
        ).toBeVisible();
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        await page.screenshot({ path: info.outputPath('pending-answer.png'), fullPage: true });
        let choice = page.getByRole('button', { name: text, exact: true });
        await expect(choice).toBeEnabled();
        expect(reads).toBe(2);
        await page.reload();
        await expect(field).toBeEnabled();
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        choice = page.getByRole('button', { name: text, exact: true });
        await expect(choice).toBeEnabled();
        await choice.scrollIntoViewIfNeeded();
        expect(
          await choice.evaluate((button) => {
            const bounds = button.getBoundingClientRect();
            return {
              inside: bounds.left >= 0 && bounds.right <= innerWidth,
              clipped: button.scrollWidth > button.clientWidth,
              height: bounds.height,
            };
          }),
        ).toMatchObject({ inside: true, clipped: false });
        expect((await choice.boundingBox())!.height).toBeGreaterThanOrEqual(44);
        await field.fill('Keep the existing draft.');
        await page.screenshot({ path: info.outputPath('saved-result.png'), fullPage: true });
        await choice.click();
        await expect.poll(() => fixture.writes.length).toBe(1);
        expect(fixture.writes[0]!.body).toEqual({
          input: text,
          stream: true,
          answer_context: saved.set.scope,
        });
        await expect(page.getByRole('region', { name: 'Next you can ask' })).toHaveCount(0);
        await expect(field).toHaveValue('Keep the existing draft.');
        expect(fixture.errors).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });
  }
}

test('edits a saved generated result through the existing composer', async ({
  page,
  baseURL,
}, info) => {
  const fixture = await open(page, baseURL!);
  await page.route('**/api/v1/agents/agt_contract/suggestions?*', (route) =>
    route.fulfill({ json: saved }),
  );
  try {
    await page.setViewportSize({ width: 320, height: 900 });
    await page.goto('/agents/agt_contract');
    await expect(page.getByRole('button', { name: text, exact: true })).toBeEnabled();
    await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
    await page.getByRole('button', { name: 'Edit suggestion 1 before sending' }).click();
    const field = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(field).toHaveValue(text);
    await expect(field).toBeFocused();
    expect(fixture.writes).toEqual([]);
    const edited = 'Explain a nightly SQLite backup schedule for my offline notes app.';
    await field.fill(edited);
    await page.screenshot({ path: info.outputPath('saved-result-edited.png'), fullPage: true });
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect.poll(() => fixture.writes.length).toBe(1);
    expect(fixture.writes[0]!.body).toEqual({
      input: edited,
      stream: true,
      answer_context: saved.set.scope,
    });
    await expect(field).toHaveValue('');
    await expect(page.getByRole('region', { name: 'Next you can ask' })).toHaveCount(0);
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

test('late answer A never reopens after a confirmed manual message', async ({ page, baseURL }) => {
  const fixture = await open(page, baseURL!);
  let release = () => {};
  const waiting = new Promise<void>((resolve) => {
    release = resolve;
  });
  let reads = 0;
  await page.route('**/api/v1/agents/agt_contract/suggestions?*', async (route) => {
    reads += 1;
    await waiting;
    await route.fulfill({ json: saved });
  });
  try {
    await page.goto('/agents/agt_contract');
    await expect.poll(() => reads).toBe(1);
    const field = page.getByRole('textbox', { name: 'Message', exact: true });
    await field.fill('Use SQLite.');
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect.poll(() => fixture.writes.length).toBe(1);
    await expect(field).toHaveValue('');
    release();
    await expect(page.getByRole('region', { name: 'Next you can ask' })).toHaveCount(0);
    await expect(field).toBeEnabled();
    expect(fixture.errors).toEqual([]);
  } finally {
    release();
    await fixture.close(page);
  }
});
