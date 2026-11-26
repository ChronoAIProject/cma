import { selectHomeTemplate } from './home-simplification.fixture';
import { createHash } from 'node:crypto';
import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';
import { captureSingleBrowserFrame } from './screen-preview';

const steps = [
  'Find tools that suit a team of 8',
  'Collect pricing from each tool’s website',
  'Compare setup effort and reporting',
  'Build a comparison table',
  'Write the one-page recommendation',
];
const report = Buffer.from(
  '# Project management tools\n\n## Recommendation\n\nChoose the tool that fits your team’s workflow.\n\n| Tool | Setup effort | Reporting |\n| --- | --- | --- |\n| Asana | Moderate | Flexible |\n| Linear | Low | Focused |\n| Trello | Low | Basic |\n\n## Next steps\n\nTry the preferred tool with one project before moving the rest of the team.',
);
const delivery = {
  id: `delivery_${'a'.repeat(64)}`,
  reference: {
    agent_id: 'agt_contract',
    name: 'Tool comparison.md',
    resource: { kind: 'live_file', path: 'comparison.md' },
    media_type: 'application/octet-stream',
    size_bytes: report.length,
    sha256: createHash('sha256').update(report).digest('hex'),
  },
  status: 'ready',
};

export async function policy(page: Page, replay = false, followups = false) {
  await page.route('**/api/v2/ui/task-experience', (route) =>
    route.fulfill({
      json: {
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: 'local-owner',
        capabilities: {
          task_home_admission: true,
          desktop_enable: true,
          suggested_followup_admission: followups,
          browser_takeover: false,
          replay_presentation: replay,
        },
      },
    }),
  );
}

export async function browser(page: Page, replay = false) {
  const capture = await page.context().newPage();
  let image: string;
  try {
    await capture.setViewportSize({ width: 1280, height: 720 });
    await capture.setContent(
      '<main style="font:24px system-ui;padding:48px;background:#f4f4f5;height:600px"><p>Local browser fixture</p><h1>Compare project tools</h1><p>Researching setup effort and reporting</p><div style="display:flex;gap:24px">' +
        ['Asana', 'Linear', 'Trello']
          .map(
            (name) =>
              `<section style="background:white;border:1px solid #ccc;border-radius:12px;padding:32px;flex:1"><h2>${name}</h2><p>Features</p><p>Setup</p><p>Reporting</p></section>`,
          )
          .join('') +
        '</div></main>',
    );
    await capture.evaluate(() => document.fonts.ready);
    image = (await capture.screenshot({ type: 'jpeg', quality: 70 })).toString('base64');
  } finally {
    await capture.close();
  }
  const feature = (state: string) => ({
    state,
    reason: 'local_visual_fixture',
    scope: 'agent_runtime',
    writable: false,
  });
  await page.route('**/api/v1/agents/agt_contract/environment', (route) =>
    route.fulfill({
      json: {
        agent_id: 'agt_contract',
        policy_revision: 1,
        runtime_incarnation: 'local-design',
        revalidate_at: null,
        files: feature('available'),
        operation_output: feature('available'),
        browser: feature('available'),
        replay: feature(replay ? 'available' : 'unsupported'),
        control: feature('unsupported'),
      },
    }),
  );
  let sequence = 0;
  await page.route('**/api/v1/agents/agt_contract/browser?*', (route) =>
    route.fulfill({
      json: {
        agent_id: 'agt_contract',
        runtime_incarnation: 'local-design',
        context_id: 'e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24',
        state: 'live',
        location: 'Local page',
        observers: 1,
        captures: sequence,
        bytes: 20000,
        min_interval_ms: 1000,
        fresh_for_ms: 3000,
        max_frame_bytes: 262144,
        max_observers: 16,
        frame: {
          sequence: ++sequence,
          captured_at: new Date().toISOString(),
          media_type: 'image/jpeg',
          width: 1280,
          height: 720,
          data: image,
        },
      },
    }),
  );
}

export function registerDesignCorrectionTaskCases() {
  for (const theme of ['light', 'dark']) {
    for (const width of [1440, 390]) {
      for (const state of ['home', 'working', 'completed'] as const) {
        test(`LOCAL design ${state} ${theme} at ${width}px preserves task context`, async ({
          page,
          baseURL,
        }, info) => {
          await page.setViewportSize({ width, height: 900 });
          await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
          const fixture = new LocalAgentV2();
          const boundary = fixture.boundary;
          try {
            boundary.agent = {
              ...boundary.agent,
              title: 'Compare project tools',
              owner_title: 'Compare project tools',
            };
            const events: LocalStartup['events'] = [
              { type: 'turn_started', turn_sequence: 1 },
              {
                type: 'owner_message',
                content:
                  'Compare three project management tools for a small team of 8 and write a one-page recommendation.',
              },
              {
                type: 'agent_message',
                content:
                  'I’ll look at price, setup effort and reporting, then write the recommendation as a document you can download.',
              },
              {
                type: 'plan_set',
                entries: steps.map((title, index) => ({
                  title,
                  status:
                    state === 'completed' || index < 2
                      ? 'completed'
                      : index === 2
                        ? 'in_progress'
                        : 'pending',
                })),
              },
              ...(state === 'completed'
                ? [
                    {
                      type: 'agent_message',
                      content: `I compared the three tools. The recommendation and comparison table are ready.\n<!--cma-deliverable:v1:${Buffer.from(JSON.stringify(delivery.reference)).toString('base64url')}-->`,
                    },
                    { type: 'turn_completed', turn_sequence: 1 },
                  ]
                : []),
            ].map((kind, index) => ({
              seq: index + 1,
              appended_at: new Date(
                Date.parse('2026-11-04T01:00:00Z') + index * 30_000,
              ).toISOString(),
              kind,
            }));
            if (state === 'completed') {
              for (const entry of events) {
                const kind = ['owner_message', 'agent_message'].includes(String(entry.kind.type))
                  ? {
                      type: 'message',
                      role: entry.kind.type === 'owner_message' ? 'user' : 'assistant',
                      content: entry.kind.content,
                    }
                  : entry.kind;
                fixture.append(kind);
                fixture.items.at(-1)!.appended_at = entry.appended_at;
              }
            }
            await fixture.route(page, baseURL!);
            await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
              route.fulfill({
                json: {
                  object: 'cma.agent.event_head',
                  schema_version: 1,
                  agent_id: 'agt_contract',
                  incarnation: 'a'.repeat(64),
                  through_cursor: cursor(fixture.items.length),
                  active_turn: state === 'working' ? (fixture.items[0] ?? null) : null,
                },
              }),
            );
            await policy(page, false, state === 'completed');
            if (state === 'working') await browser(page);
            if (state === 'completed')
              await page.route('**/api/v1/agents/agt_contract/suggestions?*', (route) =>
                route.fulfill({
                  json: {
                    status: 'ready',
                    set: {
                      id: 'local-design-followups',
                      viewerId: 'local-owner',
                      agentId: 'agt_contract',
                      answerId: 'event:5',
                      scope: { turn_sequence: 1, answer_sequence: 5 },
                      source: { kind: 'generated', version: 'local-design-example' },
                      items: [
                        'Make this into slides',
                        'Add two more tools',
                        'Draft an email to the team',
                      ].map((text, rank) => ({ id: `local-followup-${rank}`, text, rank })),
                    },
                    observation: null,
                  },
                }),
              );
            if (state === 'completed')
              await page.route('**/api/v1/agents/agt_contract/answers/**', (route) => {
                const path = new URL(route.request().url()).pathname;
                const identity = {
                  agent_id: 'agt_contract',
                  answer_id: 'event:5',
                  turn_sequence: 1,
                };
                if (path.endsWith('/download'))
                  return route.fulfill({ contentType: 'application/octet-stream', body: report });
                return route.fulfill({
                  json: path.endsWith('/deliverables')
                    ? { ...identity, items: [delivery] }
                    : { ...identity, item: delivery },
                });
              });
            await page.goto(state === 'home' ? '/' : '/agents/agt_contract');
            if (state === 'working') {
              await expect.poll(() => fixture.streams.size).toBeGreaterThan(0);
              for (const entry of events) {
                const kind = ['owner_message', 'agent_message'].includes(String(entry.kind.type))
                  ? {
                      type: 'message',
                      role: entry.kind.type === 'owner_message' ? 'user' : 'assistant',
                      content: entry.kind.content,
                    }
                  : entry.kind;
                fixture.append(kind, kind.type === 'turn_started' ? binding : null);
              }
            }
            if (state === 'home') {
              await expect(
                page.getByRole('heading', { name: /^Good (morning|afternoon|evening)\.$/ }),
              ).toBeVisible();
              await expect(page.getByLabel('GitHub repository')).not.toBeVisible();
              await page.getByLabel('Your task').fill('Compare three project management tools');
              await expect(page.getByRole('button', { name: 'Start task' })).toBeEnabled();
              await expect
                .poll(() =>
                  page
                    .locator('.frame__center')
                    .evaluate((element) => element.getBoundingClientRect().right),
                )
                .toBe(width);
              await page.evaluate(() => document.fonts.ready);
              await page.screenshot({ path: info.outputPath(`home-${width}.png`) });
              await page.getByRole('button', { name: 'Add git repo', exact: true }).click();
              await expect(page.getByRole('dialog', { name: 'Add git repo' })).toBeVisible();
              await expect(page.getByLabel('GitHub repository')).toBeFocused();
              await expect(page.getByLabel('Your task')).toHaveValue(
                'Compare three project management tools',
              );
            } else {
              const composer = page.getByRole('textbox', { name: 'Message', exact: true });
              await expect(composer).toBeVisible();
              await expect(
                page.getByRole('button', { name: 'Context usage', exact: true }),
              ).toHaveCount(0);
              await expect(page.getByRole('button', { name: /^Agent settings:/ })).toHaveAttribute(
                'title',
                /Context:/,
              );
              await composer.fill('Keep my unsent follow-up');
              const original = await composer.elementHandle();
              const plan = page.getByRole('region', { name: 'Plan', exact: true });
              if (state === 'working') {
                await expect(plan.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '2');
                await expect(plan.getByText(/^Current step 3:/)).toBeVisible();
                await page.screenshot({ path: info.outputPath(`working-${width}.png`) });
                await page
                  .getByRole('button', { name: 'Show Agent computer', exact: true })
                  .click();
                await expect(page.getByAltText('Agent browser page')).toBeVisible();
                await expect(page.getByRole('tab', { name: 'Operations' })).toHaveCount(0);
                await page.getByRole('tab', { name: 'Files' }).click();
                await page.getByRole('tab', { name: 'Screen', exact: true }).click();
                await captureSingleBrowserFrame(page, info.outputPath(`computer-${width}.png`));
                await page.getByRole('button', { name: 'Close computer' }).click();
              } else {
                await expect(
                  page.getByRole('button', { name: 'Make this into slides', exact: true }),
                ).toBeVisible();
                await expect(plan.getByRole('button', { name: 'View plan' })).toHaveAttribute(
                  'aria-expanded',
                  'false',
                );
                await page.getByRole('button', { name: 'Preview Tool comparison.md' }).click();
                const preview = page.getByRole('complementary', { name: 'Deliverable preview' });
                await expect(
                  preview.getByRole('heading', { name: 'Project management tools' }),
                ).toBeVisible();
                await expect
                  .poll(async () => (await preview.boundingBox())!.width)
                  .toBeGreaterThanOrEqual(width >= 1024 ? 500 : width - 1);
                await page.screenshot({ path: info.outputPath(`completed-${width}.png`) });
                await preview.getByRole('button', { name: 'Close preview' }).click();
              }
              await expect(composer).toHaveValue('Keep my unsent follow-up');
              expect(
                await composer.evaluate((element, previous) => element === previous, original),
              ).toBe(true);
            }
            expect(
              await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
            ).toBe(true);
            expect(boundary.writes).toEqual([]);
            expect(boundary.errors).toEqual([]);
          } catch (error) {
            console.error({
              reads: fixture.reads,
              errors: fixture.errors,
              unexpected: boundary.unexpected,
              pageErrors: boundary.errors,
            });
            if (!page.isClosed())
              await page
                .screenshot({ path: info.outputPath(`failure-${state}-${width}.png`) })
                .catch(() => {});
            throw error;
          } finally {
            await fixture.close(page);
          }
        });
      }
    }
  }
}

export function registerDesignCorrectionHomeCases() {
  for (const theme of ['light', 'dark']) {
    for (const width of [1440, 390]) {
      test(`LOCAL New agent ${theme} at ${width}px keeps repository and menu accessible`, async ({
        page,
        baseURL,
      }, info) => {
        await page.setViewportSize({ width, height: 900 });
        await page.addInitScript(
          (appearance) => localStorage.setItem('cma:appearance', appearance),
          theme,
        );
        const boundary = new LocalStartup();
        await boundary.route(page, baseURL!);
        await policy(page);
        await page.goto('/');
        const input = page.getByLabel('Your task');
        await expect(input).toBeVisible();
        await expect
          .poll(() =>
            page
              .locator('.frame__center')
              .evaluate((element) => element.getBoundingClientRect().right),
          )
          .toBe(width);
        await page.evaluate(() => document.fonts.ready);
        await expect(page.getByRole('button', { name: 'Start task' })).toBeDisabled();
        await expect(page.locator('.task-home__composer')).toHaveCSS(
          'background-color',
          theme === 'dark' ? 'rgb(20, 20, 22)' : 'rgb(255, 255, 255)',
        );
        await expect(page.locator('.task-home__composer')).toHaveCSS('border-radius', '20px');
        await expect(page.locator('.task-home__composer')).toHaveCSS(
          'border-color',
          theme === 'dark' ? 'rgb(38, 38, 42)' : 'rgb(228, 228, 231)',
        );
        await expect(page.locator('.task-home__panel')).toHaveCSS(
          'background-color',
          'rgba(0, 0, 0, 0)',
        );
        await expect(page.locator('.task-home__panel')).toHaveCSS('border-width', '0px');
        await expect(page.getByRole('button', { name: 'Start task' })).toHaveCSS(
          'background-color',
          theme === 'dark' ? 'rgb(38, 38, 42)' : 'rgb(228, 228, 231)',
        );
        expect(await input.evaluate((node) => getComputedStyle(node, '::placeholder').color)).toBe(
          theme === 'dark' ? 'rgb(133, 133, 142)' : 'rgb(82, 82, 91)',
        );
        await input.focus();
        await expect(page.locator('.task-home__composer')).toHaveCSS(
          'border-color',
          theme === 'dark' ? 'rgb(200, 245, 66)' : 'rgb(77, 124, 15)',
        );

        await expect(input).toHaveCSS('min-height', '96px');
        await expect(
          page.getByRole('heading', { name: /^Good (morning|afternoon|evening)\.$/ }),
        ).toHaveCSS('font-size', width === 1440 ? '36px' : '28px');
        await input.fill('Research a topic while retaining this independent task draft.');
        await selectHomeTemplate(page, 'Research a topic');
        await expect(page.getByLabel('Topic', { exact: true })).toBeFocused();
        await expect(page.getByRole('button', { name: 'Start task' })).toBeDisabled();
        await page.getByRole('button', { name: 'Write a task without a template' }).click();
        const keepDraft = page.getByRole('dialog', { name: 'Keep this template draft?' });
        await expect(keepDraft).toHaveCount(0);
        await expect(input).toHaveValue(
          'Research a topic while retaining this independent task draft.',
        );
        await selectHomeTemplate(page, 'Write a document');
        await page.getByLabel('Subject', { exact: true }).fill('Keep this partial document draft');
        await page.getByRole('button', { name: 'Write a task without a template' }).click();
        await expect(keepDraft).toBeVisible();
        await keepDraft.getByRole('button', { name: 'Switch and keep draft' }).click();
        await expect(input).toHaveValue(
          'Research a topic while retaining this independent task draft.',
        );
        await expect(page.getByRole('button', { name: 'Start task' })).toBeEnabled();
        await expect(page.getByRole('button', { name: 'Start task' })).toHaveCSS(
          'background-color',
          'rgb(200, 245, 66)',
        );
        const trigger = page.getByRole('button', { name: /^Cloud computer/ });
        await trigger.click();
        const menu = page.getByRole('menu', { name: 'Run this task on' });
        await expect(menu).toHaveCSS('opacity', '1');
        await expect(menu).toHaveCSS('border-radius', '18px');
        await expect(menu).toHaveCSS('width', width < 352 ? `${width - 32}px` : '320px');
        await expect(menu.getByRole('menuitemradio', { name: 'Cloud computer' })).toBeChecked();
        await expect(menu.getByRole('menuitemcheckbox', { name: 'Agent browser' })).toHaveCount(0);
        await page.keyboard.press('ArrowDown');
        await expect(menu.getByRole('menuitemcheckbox', { name: 'Shared desktop' })).toBeFocused();
        await page.screenshot({ path: info.outputPath(`menu-${theme}-${width}.png`) });
        await page.keyboard.press('Escape');
        await expect(trigger).toBeFocused();
        await page.getByRole('button', { name: 'Add git repo' }).click();
        const dialog = page.getByRole('dialog', { name: 'Add git repo' });
        await expect(dialog).toHaveAttribute('aria-modal', 'true');
        await expect(dialog.locator('..')).toHaveCSS('opacity', '1');
        await expect(dialog).toHaveCSS('border-radius', '26px');
        await expect(dialog).toHaveCSS('width', width < 512 ? `${width - 32}px` : '480px');
        await expect(page.locator('.modal__mask')).toHaveCSS('backdrop-filter', 'blur(8px)');
        const repo = dialog.getByLabel('GitHub repository');
        await expect(repo).toBeFocused();
        await repo.fill('owner/project.git/');
        await dialog.getByLabel('Repository ref').fill('bad..ref');
        await expect(dialog.getByRole('button', { name: 'Add repo' })).toBeDisabled();
        await dialog.getByLabel('Repository ref').fill('feature/landing');
        await expect(dialog.getByRole('button', { name: 'Add repo' })).toBeEnabled();
        await page.mouse.move(0, 0);
        await expect(dialog.getByRole('button', { name: 'Add repo' })).toHaveCSS(
          'background-color',
          'rgb(200, 245, 66)',
        );
        await dialog.getByRole('button', { name: 'Close repository dialog' }).focus();
        await page.keyboard.press('Tab');
        await expect(repo).toBeFocused();
        await page.keyboard.press('Shift+Tab');
        await expect(dialog.getByRole('button', { name: 'Close repository dialog' })).toBeFocused();
        await page.screenshot({ path: info.outputPath(`repository-${theme}-${width}.png`) });
        await repo.press('Enter');
        await expect(dialog).toHaveCount(0);
        await expect(page.getByRole('button', { name: 'owner/project' })).toBeFocused();
        await page.getByRole('button', { name: 'Remove repository' }).click();
        await expect(page.getByRole('button', { name: 'Add git repo' })).toBeFocused();
        if (width < 600) {
          for (const button of await page
            .getByRole('group', { name: 'Task templates' })
            .getByRole('button')
            .all())
            expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
        }
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        expect(boundary.writes).toEqual([]);
      });
    }
  }
}
