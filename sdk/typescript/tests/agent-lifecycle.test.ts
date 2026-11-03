import { expect, it } from 'vitest';
import { transport } from './resource-fixtures.js';
import type { AgentLifecycleCreateRequest } from '../src/index.js';

it('preserves exact initial task bytes and the original key on lifecycle replay', async () => {
  const test = transport();
  test.answer({ id: 'operation-task', status: 'provisioning' }, 202);
  const request: AgentLifecycleCreateRequest = {
    name: 'New task',
    initial_message: '  比较三个工具\nKeep these bytes.  ',
  };
  await test.client.agents.createLifecycle(request, { idempotencyKey: 'initial-task' });
  await test.client.agents.createLifecycle(request, { idempotencyKey: 'initial-task' });
  expect(
    test.calls.map((call) => ({
      body: JSON.parse(String(call.init.body)),
      key: new Headers(call.init.headers).get('Idempotency-Key'),
    })),
  ).toEqual([
    {
      body: { name: 'New task', initial_message: '  比较三个工具\nKeep these bytes.  ' },
      key: 'initial-task',
    },
    {
      body: { name: 'New task', initial_message: '  比较三个工具\nKeep these bytes.  ' },
      key: 'initial-task',
    },
  ]);
});

it('retains the selected Workspace and exact inline intent when replaying Agent creation', async () => {
  const test = transport();
  test.answer({ id: 'operation-context', status: 'provisioning' }, 202);
  const request = {
    name: 'Second Agent',
    workspace_id: 'wk_existing',
    profile: { model: 'model', system_prompt: 'Continue the project.' },
  };
  for (let attempt = 0; attempt < 2; attempt += 1) {
    expect(
      await test.client.agents.createLifecycle(request, { idempotencyKey: 'exact-context' }),
    ).toEqual({ id: 'operation-context', status: 'provisioning' });
  }
  expect(test.calls).toHaveLength(2);
  for (const call of test.calls) {
    expect(call.url).toBe('https://cma.example.test/api/v1/agents');
    expect(call.init.method).toBe('POST');
    expect(new Headers(call.init.headers).get('Idempotency-Key')).toBe('exact-context');
    expect(JSON.parse(String(call.init.body))).toEqual(request);
  }
});
