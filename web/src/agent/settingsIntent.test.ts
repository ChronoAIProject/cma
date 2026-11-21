import { beforeEach, expect, it, vi } from 'vitest';
import { agentTransportJournal, clearAgentTransport } from './transport/journal';
import {
  parsePreferencesIntentBody,
  parseSettingsIntentBody,
  settingsIntentJournal,
} from './settingsIntent';

const proposal = {
  model: 'test-model',
  reasoning_effort: 'high',
  permission_mode: 'guarded',
} as const;

const settings = (viewer = 'viewer-one', agent = 'agent-one') =>
  agentTransportJournal(sessionStorage, viewer, agent).settingsIntent(
    'settings',
    parseSettingsIntentBody,
  );

beforeEach(() => sessionStorage.clear());

it('recovers the body and key after reopening and protects a different Agent and editor', () => {
  const journal = settings();
  const pending = { key: crypto.randomUUID(), startedAt: Date.now(), body: proposal };
  journal.reserve(pending);
  expect(settings().read()).toEqual(pending);
  expect(settings('viewer-one', 'agent-two').read()).toBeUndefined();
  expect(
    agentTransportJournal(sessionStorage, 'viewer-one', 'agent-one')
      .settingsIntent('prompt', (body) => parsePreferencesIntentBody(body, 'prompt'))
      .read(),
  ).toBeUndefined();
  expect(() => journal.reserve({ ...pending, key: crypto.randomUUID() })).toThrow();
  expect(() =>
    journal.reserve({ ...pending, body: { ...proposal, reasoning_effort: 'low' } }),
  ).toThrow();
  journal.complete(crypto.randomUUID());
  expect(journal.read()).toEqual(pending);
  journal.complete(pending.key);
  expect(settings().read()).toBeUndefined();
});

it('rejects an earlier identity after account change or sign-out', () => {
  const journal = settings();
  const pending = { key: crypto.randomUUID(), startedAt: Date.now(), body: proposal };
  journal.reserve(pending);
  expect(settings('viewer-two').read()).toBeUndefined();
  expect(() => journal.read()).toThrow();
  expect(() => journal.complete(pending.key)).toThrow();
  const second = settings('viewer-two');
  clearAgentTransport(sessionStorage);
  expect(() => second.reserve(pending)).toThrow();
  expect(settings().read()).toBeUndefined();
});

it('retains the retry deadline alongside a refused or interrupted response', () => {
  const journal = settings();
  const pending = { key: crypto.randomUUID(), startedAt: Date.now(), body: proposal };
  const retryAt = pending.startedAt + 2_100;
  journal.reserve(pending);
  journal.defer(crypto.randomUUID(), retryAt);
  expect(settings().read()).toEqual(pending);
  journal.defer(pending.key, retryAt);
  expect(settings().read()).toEqual({ ...pending, retryAt });
});

it('never submits an expired stored intent under a key whose server receipt may have expired', () => {
  vi.useFakeTimers();
  try {
    const journal = settings();
    const pending = { key: crypto.randomUUID(), startedAt: Date.now(), body: proposal };
    journal.reserve(pending);
    vi.setSystemTime(pending.startedAt + 24 * 60 * 60 * 1000);
    expect(() => settings().reserve(pending)).toThrow();
    expect(settings().read()).toEqual(pending);
  } finally {
    vi.useRealTimers();
  }
});

it('refuses invalid settings bodies before reserving a request identity', () => {
  const journal = settings();
  for (const body of [
    { ...proposal, model: ' ' },
    { ...proposal, model: 'x'.repeat(257) },
    { ...proposal, reasoning_effort: 'unknown' },
    { ...proposal, permission_mode: 'unknown' },
    { ...proposal, pending: true },
    ['test-model', 'low', 'guarded'],
  ]) {
    expect(parseSettingsIntentBody(body)).toBeUndefined();
  }
  expect(journal.read()).toBeUndefined();
});

it('preserves exact prompt bytes and rejects payloads for the other preference editor', () => {
  const prompt = { expected_revision: 0, system_prompt: '  Instructions\nkeep whitespace  ' };
  const rename = { expected_revision: 2, title: 'Draft title' };
  expect(parsePreferencesIntentBody(prompt, 'prompt')).toEqual(prompt);
  expect(parsePreferencesIntentBody(rename, 'rename')).toEqual(rename);
  expect(parsePreferencesIntentBody(prompt, 'rename')).toBeUndefined();
  expect(parsePreferencesIntentBody(rename, 'prompt')).toBeUndefined();
  for (const body of [
    { ...prompt, expected_revision: -1 },
    { ...prompt, expected_revision: '0' },
    { ...prompt, expected_revision: 1.5 },
    { ...prompt, system_prompt: '界'.repeat(11000) },
    { ...prompt, title: rename.title },
  ]) {
    expect(parsePreferencesIntentBody(body, 'prompt')).toBeUndefined();
  }
  expect(parsePreferencesIntentBody({ ...rename, title: ' ' }, 'rename')).toBeUndefined();
  expect(
    parsePreferencesIntentBody({ ...rename, title: '界'.repeat(65) }, 'rename'),
  ).toBeUndefined();
});

it('does not overwrite a corrupt intent that might still refer to an unconfirmed write', () => {
  const scope = 'synthetic-editor';
  const journal = settingsIntentJournal(sessionStorage, scope, vi.fn(), parseSettingsIntentBody);
  const pending = { key: crypto.randomUUID(), startedAt: Date.now(), body: proposal };
  for (const raw of [
    '{',
    '[]',
    'x'.repeat(200001),
    JSON.stringify({ ...pending, key: 'invalid' }),
    JSON.stringify({ ...pending, startedAt: -1 }),
    JSON.stringify({ ...pending, retryAt: pending.startedAt - 1 }),
  ]) {
    sessionStorage.setItem(scope, raw);
    expect(() => journal.reserve(pending)).toThrow();
    expect(sessionStorage.getItem(scope)).toBe(raw);
  }
});
