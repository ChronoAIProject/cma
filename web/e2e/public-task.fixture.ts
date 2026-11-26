import { createHash } from 'node:crypto';
import type { Page } from '@playwright/test';
import type { Harness } from './harness';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { publicApiPath } from './public-api-path';
import { taskAdmission } from './resource-lifecycle.fixture';

export async function taskFixture(page: Page, baseURL: string, loseResponse = false) {
  const boundary = new LocalStartup();
  boundary.sandbox = { ...boundary.sandbox, status: 'active' };
  boundary.agent = {
    ...boundary.agent,
    status: 'ready',
    sandbox: { ...boundary.agent.sandbox, status: 'active' },
  };
  const message = 'Reply with your working directory.';
  const answer = 'Synthetic completed answer';
  boundary.events = [
    { seq: 1, appended_at: localTime, kind: { type: 'turn_started', turn_sequence: 1 } },
    { seq: 2, appended_at: localTime, kind: { type: 'owner_message', content: message } },
    { seq: 3, appended_at: localTime, kind: { type: 'agent_message', content: answer } },
    { seq: 4, appended_at: localTime, kind: { type: 'turn_completed', turn_sequence: 1 } },
  ];
  const transcript = {
    agent_id: boundary.agent.id,
    through_seq: 4,
    turns: [
      {
        turn_sequence: 1,
        started_at: localTime,
        owner_message: message,
        agent_messages: [answer],
        messages: [
          { sequence: 2, role: 'user', content: message, appended_at: localTime },
          { sequence: 3, role: 'assistant', content: answer, appended_at: localTime },
        ],
        outcome: { type: 'completed', at: localTime },
      },
    ],
  };
  const operation = {
    id: 'inline-operation',
    status: 'succeeded',
    phase: 'ready',
    agent_id: boundary.agent.id,
    events: [],
  };
  const commands: Harness['commands'] = [];
  const bodies: unknown[] = [];
  const reads = (path: string): unknown => {
    if (path === `/api/v1/agents/${boundary.agent.id}/transcript`) return transcript;
    if (path === `/api/v1/agents/${boundary.agent.id}`) return boundary.agent;
    if (path === '/api/v1/agent-lifecycle/inline-operation') return operation;
    throw new Error(`Unexpected synthetic task read: ${path}`);
  };
  await boundary.route(page, baseURL);
  await taskAdmission(page);
  await page.route('**/*', async (route) => {
    const request = route.request();
    const path = publicApiPath(new URL(request.url()).pathname);
    if (request.method() === 'POST' && path === '/api/v1/agents') {
      bodies.push(request.postDataJSON());
      commands.push({
        path,
        method: 'POST',
        key: request.headers()['idempotency-key'],
        bodyHash: createHash('sha256')
          .update(request.postData() ?? '')
          .digest('hex'),
      });
      if (loseResponse && bodies.length === 1) return route.abort('connectionfailed');
      return route.fulfill({
        status: 202,
        json: { id: operation.id, status: 'succeeded', agent_id: boundary.agent.id },
      });
    }
    if (path.endsWith('/transcript') || path === '/api/v1/agent-lifecycle/inline-operation')
      return route.fulfill({ json: reads(path) });
    return route.fallback();
  });
  const harness = {
    config: { baseURL },
    fixture: { accounts: { owner: { subject: 'local-owner' } } },
    commands,
    journal: { observe: () => {} },
    flush: async () => {},
    get: async (_page: Page, path: string) => reads(path),
  } as unknown as Harness;
  return {
    harness,
    boundary,
    bodies,
    message,
    workspace: { id: boundary.workspace.id, path: boundary.workspace.path },
  };
}
