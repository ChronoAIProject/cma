// @vitest-environment node
import { expect, it } from 'vitest';
import { agent, transport } from './resource-fixtures.js';
import type { AgentLifecycleCreateRequest } from '../index.js';

it.each([undefined, '2026-11-23T08:00:00Z'])(
  'preserves the optional durable acceptance time %s without deriving one from progress',
  async (accepted_at) => {
    const test = transport();
    const operation = {
      id: 'original-operation',
      status: 'running',
      phase: 'agent_startup',
      events: [],
      ...(accepted_at === undefined ? {} : { accepted_at }),
    };
    test.answer(operation);
    for (let attempt = 0; attempt < 2; attempt += 1)
      expect(await test.client.agents.readLifecycle(operation.id)).toEqual(operation);
  },
);

it.each([null, 123, 'not a timestamp'])(
  'rejects an invalid acceptance time %j',
  async (accepted_at) => {
    const test = transport();
    test.answer({
      id: 'original-operation',
      status: 'running',
      phase: 'agent_startup',
      events: [],
      accepted_at,
    });
    await expect(test.client.agents.readLifecycle('original-operation')).rejects.toThrow();
  },
);

it.each(['inferred', true, { mode: 'automatic' }])(
  'rejects invalid title mode %j before admitting a lifecycle request',
  async (title_mode) => {
    const test = transport();
    test.answer({ id: 'unexpected-operation', status: 'provisioning' }, 202);
    expect(() =>
      test.client.agents.createLifecycle(
        { name: 'New task', title_mode } as AgentLifecycleCreateRequest,
        { idempotencyKey: 'invalid-title-mode' },
      ),
    ).toThrow('title_mode');
    expect(test.calls).toHaveLength(0);
  },
);

it('reads the actual checkout separately from the requested ref and upstream source commit', async () => {
  const test = transport();
  test.answer({
    ...agent,
    workspace: {
      ...agent.workspace,
      resolved_commit: 'b'.repeat(40),
      source: {
        type: 'repository_snapshot',
        url: 'https://github.com/example/project',
        reference: 'proof/fixed',
        source_commit: 'a'.repeat(40),
      },
    },
  });
  const detail = await test.client.agents.read(agent.id);
  expect(detail.workspace.resolved_commit).toBe('b'.repeat(40));
  expect(detail.workspace.source).toEqual({
    type: 'repository_snapshot',
    url: 'https://github.com/example/project',
    reference: 'proof/fixed',
    source_commit: 'a'.repeat(40),
  });
});

it.each([
  {},
  { desktop: true },
  { machine_included: true },
  { desktop: true, machine_included: true },
  { title_mode: 'automatic' as const },
  { title_mode: 'named' as const },
])(
  'preserves exact initial task bytes, desktop intent %j and the original key on lifecycle replay',
  async (desktopIntent) => {
    const test = transport();
    test.answer({ id: 'operation-task', status: 'provisioning' }, 202);
    const request: AgentLifecycleCreateRequest = {
      name: 'New task',
      initial_message: '  比较三个工具\nKeep these bytes.  ',
      ...desktopIntent,
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
        body: {
          name: 'New task',
          initial_message: '  比较三个工具\nKeep these bytes.  ',
          ...desktopIntent,
        },
        key: 'initial-task',
      },
      {
        body: {
          name: 'New task',
          initial_message: '  比较三个工具\nKeep these bytes.  ',
          ...desktopIntent,
        },
        key: 'initial-task',
      },
    ]);
  },
);

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

it.each([1, 2] as const)(
  'preserves version %s template fields and compiled text exactly on replay',
  async (version) => {
    const test = transport();
    test.answer({ id: 'template-operation', status: 'provisioning' }, 202);
    const body: AgentLifecycleCreateRequest = {
      name: 'Task',
      template: { id: 'research', version, fields: { topic: '  Synthetic topic  ' } },
      initial_message:
        version === 1
          ? 'Research Synthetic topic and summarize the key findings, with links to sources.'
          : 'Research the following topic:\n\nSynthetic topic\n\nSummarize the key findings, with links to sources.',
    };
    for (let attempt = 0; attempt < 2; attempt += 1)
      await test.client.agents.createLifecycle(body, { idempotencyKey: 'template-key' });
    expect(test.calls.map((call) => JSON.parse(String(call.init.body)))).toEqual([body, body]);
  },
);
