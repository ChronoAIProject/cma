/** @vitest-environment node */
import { expect, it } from 'vitest';
import { ApiError, ProtocolError } from '@chronoai/cma-sdk';
import { restTransport, NOW } from '../test/rest';
import { createRestWebClient } from './rest';

const facts = { agent_id: 'agt_one', created_at: NOW, creation_origin: 'legacy_unknown' };

it('reads minimal authorized provenance and shared source through the public SDK', async () => {
  const test = restTransport();
  const client = createRestWebClient(test.sdk);
  const controller = new AbortController();
  test.reply(facts);
  expect(await client.readAgentProvenance('agt_one', { signal: controller.signal })).toEqual(facts);
  expect(test.fetch.mock.calls[0]?.[0]).toBe('/api/v1/agents/agt_one/provenance');
  const shared = { access: 'shared', agent_id: 'agt_one', origin: { type: 'trigger' } };
  test.reply(shared);
  expect(await client.readAgentSource('agt_one')).toEqual(shared);
  expect(test.fetch.mock.calls[1]?.[0]).toBe('/api/v1/agents/agt_one/source');
});

it.each([
  { ...facts, agent_id: 'agt_other' },
  { ...facts, creation_origin: 'invented' },
])('rejects a mismatched or malformed provenance projection', async (reply) => {
  const test = restTransport();
  test.reply(reply);
  await expect(createRestWebClient(test.sdk).readAgentProvenance('agt_one')).rejects.toBeInstanceOf(
    ProtocolError,
  );
});

it.each([403, 404])(
  'preserves provenance refusal %s without a legacy-source fallback',
  async (status) => {
    const test = restTransport();
    test.reply({ status, code: 'not_found', detail: 'Unavailable' }, status);
    await expect(
      createRestWebClient(test.sdk).readAgentProvenance('agt_one'),
    ).rejects.toBeInstanceOf(ApiError);
    expect(test.fetch).toHaveBeenCalledOnce();
  },
);
