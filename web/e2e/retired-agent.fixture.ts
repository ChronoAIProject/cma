import type { Page } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { taskAdmission } from './resource-lifecycle.fixture';

export const originalRequest = '  Original request 中文\nPreserve these exact bytes.  ';
export const retainedAnswer = 'Recorded answer before an independent failure.';

export async function retiredFixture(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  boundary.sandbox = { ...boundary.sandbox, status: 'retired' };
  boundary.agent = {
    ...boundary.agent,
    owner_title: 'Retained task',
    status: 'stopped',
    sandbox: { ...boundary.agent.sandbox, status: 'retired' },
  };
  const transcript = {
    agent_id: boundary.agent.id,
    through_seq: 4,
    turns: [
      {
        turn_sequence: 1,
        started_at: localTime,
        owner_message: originalRequest,
        agent_messages: [retainedAnswer],
        messages: [
          { sequence: 2, role: 'user', content: originalRequest, appended_at: localTime },
          { sequence: 3, role: 'assistant', content: retainedAnswer, appended_at: localTime },
        ],
        outcome: {
          type: 'failed',
          at: localTime,
          failure: { class: 'conversation_output_gap', detail: 'Earlier independent output gap.' },
        },
      },
    ],
  };
  const reads: string[] = [];
  const runtime: string[] = [];
  let historyError = false;
  let originalAvailable = true;
  await boundary.route(page, baseURL);
  await taskAdmission(page);
  await page.route('**/api/**', (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    reads.push(`${request.method()} ${path}`);
    if (
      /\/(events|resources|environment|prepare|head|queued|usage|settings|models|skills|run)$/.test(
        path,
      ) ||
      path.includes('/agui/')
    )
      runtime.push(`${request.method()} ${path}`);
    if (path === '/api/v1/sandboxes') return route.fulfill({ json: { items: [] } });
    if (path === `/api/v1/agents/${boundary.agent.id}/transcript`)
      return historyError
        ? route.fulfill({
            status: 503,
            json: { code: 'unavailable', detail: 'Synthetic history unavailable.' },
          })
        : route.fulfill({ json: transcript });
    if (path === `/api/v1/agents/${boundary.agent.id}/original-request`)
      return route.fulfill({
        json: originalAvailable
          ? {
              agent_id: boundary.agent.id,
              status: 'ready',
              original_request: {
                text: originalRequest,
                source: 'admitted_first_task',
                turn_sequence: 1,
              },
            }
          : { agent_id: boundary.agent.id, status: 'unavailable', original_request: null },
      });
    return route.fallback();
  });
  return {
    boundary,
    reads,
    runtime,
    transcript,
    historyError: (value: boolean) => {
      historyError = value;
    },
    originalAvailable: (value: boolean) => {
      originalAvailable = value;
    },
  };
}
