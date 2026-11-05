import { expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import {
  agent,
  response,
  fixture,
  intent,
  json,
  harness,
} from './response-controls-v2-fixtures.js';

it.each([
  ['object', 'foreign'],
  ['response.receipt.agent_id', 'foreign'],
  ['response.receipt.response_id', 'foreign'],
  ['target.response_id', 'foreign'],
  ['target.turn_sequence', 9],
  ['target.turn_sequence', 0],
  ['agent_revision', Number.MAX_SAFE_INTEGER + 1],
  ['target', null],
  ['runtime_support.cancel', 'yes'],
  ['response.status', 'unknown'],
  ['response.delivery', 'unknown'],
  ['response.failure', 'secret'],
  ['response.receipt.command_id', ''],
  ['response.observed_cursor', -1],
  ['response.receipt.event_cursor', 0.5],
  ['response.actor', 'forged'],
])('rejects malformed/foreign control observation %s', async (path, value) => {
  const body = fixture('owner-observe');
  const parts = String(path).split('.');
  let selected = body;
  for (const part of parts.slice(0, -1)) selected = selected[part] as Record<string, unknown>;
  selected[parts.at(-1)!] = value;
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(body));
  await expect(t.api.observeControls(agent, response)).rejects.toBeInstanceOf(ProtocolError);
});

it('requires explicit null fields and rejects impossible active queued/terminal observations', async () => {
  for (const mode of ['missing-null', 'queued-active', 'terminal-active']) {
    const body = fixture(mode === 'queued-active' ? 'queued-observe' : 'owner-observe');
    const source = body.response as Record<string, unknown>;
    if (mode === 'missing-null') delete source.failure;
    else if (mode === 'queued-active') body.target_active = true;
    else source.status = 'completed';
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(body));
    await expect(t.api.observeControls(agent, response)).rejects.toBeInstanceOf(ProtocolError);
  }
});

it.each([
  ['target.response_id', 'other'],
  ['target.turn_sequence', 8],
  ['expected_agent_revision', 8],
  ['control.receipt.agent_id', 'other'],
  ['control.receipt.response_id', response],
  ['control.turn_sequence', 8],
  ['control.delivery', 'write_confirmed'],
  ['control.observed_cursor', 8],
  ['control.failure', 'queue_timeout'],
])('rejects control receipt that changes the retained request: %s', async (path, value) => {
  const body = fixture('cancel');
  const parts = String(path).split('.');
  let selected = body;
  for (const part of parts.slice(0, -1)) selected = selected[part] as Record<string, unknown>;
  selected[parts.at(-1)!] = value;
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(body, 202));
  await expect(
    t.api.submitControl(agent, response, intent, { idempotencyKey: 'retained-key' }),
  ).rejects.toBeInstanceOf(ProtocolError);
});

it('rejects a different command returned by the status endpoint', async () => {
  const t = harness();
  t.fetch.mockResolvedValueOnce(json(fixture('source-status')));
  await expect(t.api.readCommand('cmd_cancel_contract')).rejects.toBeInstanceOf(ProtocolError);
});

it.each(['credential_setup_required', 'connections_required'])(
  'preserves typed revival failure %s',
  async (failure) => {
    const body = fixture('queued-observe');
    const source = body.response as Record<string, unknown>;
    source.status = 'failed';
    source.failure = failure;
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(body));
    expect((await t.api.observeControls(agent, response)).response.failure).toBe(failure);
  },
);
