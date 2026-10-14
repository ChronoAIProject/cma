import { expect, it } from 'vitest';
import {
  ProtocolError,
  ValidationError,
  type AgentModels,
  type AgentSkills,
  type InstallAgentSkillRequest,
} from '../src/index.js';
import { transport } from './resource-fixtures.js';

const skill = { id: 'research', version: '1.0' } satisfies InstallAgentSkillRequest;
const pending = {
  items: [{ ...skill, source: 'ornn', status: 'pending' }],
  pending: true,
  can_install: false,
  unavailable_reason: 'Skills can be installed when this Agent is ready and idle.',
} satisfies AgentSkills;

it('preserves per-model availability and requires runtime availability evidence', async () => {
  const test = transport();
  const models = {
    models: [
      { id: 'model-one', label: 'First model', available: true },
      { id: 'model-two', label: 'Second model', available: false },
    ],
  } satisfies AgentModels;
  test.answer(models);
  expect(await test.client.agents.models('agent/one')).toEqual(models);
  expect(test.calls[0]?.url).toBe('https://cma.example.test/api/v1/agents/agent%2Fone/models');
  expect(test.calls[0]?.init.method).toBe('GET');
  test.answer({ models: [{ id: 'model-one', label: 'First model' }] });
  await expect(test.client.agents.models('agent-one')).rejects.toBeInstanceOf(ProtocolError);
});

it('replays exact skill selection and key, then observes runtime-confirmed installation', async () => {
  const test = transport();
  test.answer(pending, 202);
  for (let attempt = 0; attempt < 2; attempt++)
    expect(
      await test.client.agents.installSkill('agent/one', skill, {
        idempotencyKey: 'install-research',
      }),
    ).toEqual(pending);
  for (const call of test.calls) {
    expect(call.url).toBe('https://cma.example.test/api/v1/agents/agent%2Fone/skills');
    expect(call.init.method).toBe('POST');
    expect(JSON.parse(String(call.init.body))).toEqual(skill);
    expect(new Headers(call.init.headers).get('Idempotency-Key')).toBe('install-research');
  }
  const installed = {
    items: [{ ...skill, name: 'Research', source: 'ornn', status: 'installed' }],
    pending: false,
    can_install: true,
  } satisfies AgentSkills;
  test.answer(installed);
  expect(await test.client.agents.skills('agent/one')).toEqual(installed);
  expect(test.calls.at(-1)?.init.method).toBe('GET');
});

it('preserves runtime capability refusal and individual installation failures', async () => {
  const test = transport();
  const unavailable = {
    items: [{ ...skill, source: 'ornn', status: 'failed', error: 'Archive unavailable' }],
    pending: false,
    can_install: false,
    unavailable_reason: 'This Agent runtime does not support installing skills yet.',
  } satisfies AgentSkills;
  test.answer(unavailable);
  expect(await test.client.agents.skills('one')).toEqual(unavailable);
  for (const invalid of [
    { items: [], pending: false },
    { ...unavailable, can_install: 'yes' },
    { ...unavailable, items: [{ ...skill, source: 'ornn', status: 'accepted' }] },
  ]) {
    test.answer(invalid);
    await expect(test.client.agents.skills('one')).rejects.toBeInstanceOf(ProtocolError);
  }
});

it('rejects invalid skill selection, extra fields and missing keys before transport', () => {
  const test = transport();
  const invalidSelections = [
    ...['', '.hidden', '../skill', 'nyxid', 'ornn-agent-manual-cli', 'a'.repeat(65)].map((id) => ({
      ...skill,
      id,
    })),
    ...['', '@latest', ' padded ', '1.0\n', 'é'.repeat(129)].map((version) => ({
      ...skill,
      version,
    })),
    { ...skill, source: 'ornn' },
  ];
  for (const body of invalidSelections)
    expect(() =>
      test.client.agents.installSkill('one', body, {
        idempotencyKey: 'skill-one',
      }),
    ).toThrow(ValidationError);
  expect(() =>
    test.client.agents.installSkill('one', skill, {
      idempotencyKey: '',
    }),
  ).toThrow(ValidationError);
  expect(test.calls).toEqual([]);
});
