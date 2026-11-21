import { test, expect, type Page } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

const initialTask = 'Compare local build tools';
const answer = 'The comparison is complete.';

async function expectTitle(page: Page, title: string) {
  await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible();
  await expect(page).toHaveTitle(`${title} · CMA`);
  await expect(page.getByRole('link', { name: new RegExp(title) })).toBeVisible();
}

for (const source of ['generated', 'fallback'] as const) {
  test(`LOCAL persisted ${source} title follows task creation, completed turn, reload and reopening`, async ({
    page,
    context,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    boundary.agent = { ...boundary.agent, title: undefined };
    boundary.workspace = {
      ...boundary.workspace,
      agents: boundary.workspace.agents.map((agent) => ({ ...agent, title: undefined })),
    };
    const writes: { body: unknown; key?: string }[] = [];
    const messages: unknown[] = [];
    let completed = false;
    const routePage = async (target: Page) => {
      await boundary.route(target, baseURL!);
      await target.route('**/openapi.json', (route) =>
        route.fulfill({
          json: {
            openapi: '3.1.0',
            info: { title: 'CMA', version: '1' },
            paths: {},
            'x-cma-agent-title-mode': true,
            components: {
              schemas: {
                AgentLifecycleCreateRequest: {
                  properties: { initial_message: { type: 'string' } },
                },
              },
            },
          },
        }),
      );
      await target.route('**/api/v2/ui/task-experience', (route) =>
        route.fulfill({
          json: {
            object: 'cma.ui.task_experience',
            schema_version: 1,
            account_id: 'local-owner',
            capabilities: {
              task_home_admission: true,
              desktop_enable: false,
              suggested_followup_admission: false,
              browser_takeover: false,
              replay_presentation: false,
            },
          },
        }),
      );
      await target.route('**/api/v1/agui/run', (route) => {
        const request = route.request().postDataJSON();
        messages.push(request);
        return route.fulfill({
          contentType: 'text/event-stream',
          body: ['RUN_STARTED', 'RUN_FINISHED']
            .map(
              (type) =>
                `data: ${JSON.stringify({ type, threadId: request.threadId, runId: request.runId })}\n\n`,
            )
            .join(''),
        });
      });
      await target.route('**/api/agent-manager/v1/agents', (route) => {
        writes.push({
          body: route.request().postDataJSON(),
          key: route.request().headers()['idempotency-key'],
        });
        return route.fulfill({ status: 202, json: { id: 'title-operation', status: 'queued' } });
      });
      await target.route('**/api/agent-manager/v1/agent-lifecycle/title-operation', (route) =>
        route.fulfill({
          json: {
            id: 'title-operation',
            status: 'succeeded',
            phase: 'agent_startup',
            agent_id: boundary.agent.id,
            events: [],
          },
        }),
      );
      await target.route('**/api/v1/agents/agt_contract/transcript*', (route) =>
        route.fulfill({
          json: {
            agent_id: boundary.agent.id,
            through_seq: completed ? boundary.events.at(-1)?.seq : 0,
            turns: completed
              ? [
                  {
                    turn_sequence: 1,
                    owner_message: initialTask,
                    agent_messages: [answer],
                    outcome: { type: 'completed', at: localTime },
                    started_at: localTime,
                    messages: [
                      { sequence: 2, role: 'user', content: initialTask, appended_at: localTime },
                      { sequence: 3, role: 'assistant', content: answer, appended_at: localTime },
                    ],
                  },
                ]
              : [],
          },
        }),
      );
    };
    await routePage(page);
    await page.clock.install();
    await page.goto('/');
    await page.getByLabel('Your task').fill(initialTask);
    await page.getByRole('button', { name: 'Start task', exact: true }).click();
    await expect(page).toHaveURL(/\/agents\/agt_contract$/);
    expect(writes).toEqual([
      {
        body: { name: 'New task', title_mode: 'automatic', initial_message: initialTask },
        key: expect.any(String),
      },
    ]);
    const composer = page.getByRole('textbox', { name: 'Message', exact: true });
    await expect(composer).toBeEnabled();
    await composer.fill('Keep this unsent draft');
    completed = true;
    const title = source === 'generated' ? 'Build tool comparison' : initialTask;
    boundary.events = [
      { seq: 1, appended_at: localTime, kind: { type: 'turn_started', turn_sequence: 1 } },
      { seq: 2, appended_at: localTime, kind: { type: 'owner_message', content: initialTask } },
      { seq: 3, appended_at: localTime, kind: { type: 'agent_message', content: answer } },
      { seq: 4, appended_at: localTime, kind: { type: 'turn_completed', turn_sequence: 1 } },
      { seq: 5, appended_at: localTime, kind: { type: 'title_set', title, source } },
    ];
    boundary.agent = { ...boundary.agent, title, updated_at: '2026-09-05T00:01:00Z' };
    boundary.workspace = {
      ...boundary.workspace,
      agents: boundary.workspace.agents.map((agent) => ({
        ...agent,
        title,
        task_status: 'completed',
        updated_at: boundary.agent.updated_at,
      })),
    };
    await page.clock.runFor(15_000);
    await expectTitle(page, title);
    await expect(composer).toHaveValue('Keep this unsent draft');
    expect(writes).toHaveLength(1);
    await page.reload();
    await expectTitle(page, title);
    await expect(page.getByText(answer, { exact: true })).toBeVisible();
    const reopened = await context.newPage();
    await routePage(reopened);
    await reopened.goto('/agents/agt_contract');
    await expectTitle(reopened, title);
    await expect(reopened.getByText(answer, { exact: true })).toBeVisible();
    await reopened.close();
    await page.goto('/');
    await page.getByRole('link', { name: new RegExp(title) }).click();
    await expectTitle(page, title);
    const nextMessage = page.getByRole('textbox', { name: 'Message', exact: true });
    await nextMessage.fill('Compare another tool');
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect.poll(() => messages.length).toBe(1);
    await expectTitle(page, title);
    expect(writes).toHaveLength(1);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
