import {randomUUID} from 'node:crypto';
import {assertOracle, assertResponse, check, poll, terminal} from '../cma/core.mjs';

// The Responses fixture observes the actual request from the pinned runtime.
// Its oracle exposes counts alone, so instructions and provider payloads stay private.
export async function probeConfiguration({api, adapter, record}, configuration) {
  const {agentId} = record;
  const caseId = 'configuration-' + randomUUID();
  const marker = 'WS4_' + randomUUID().replaceAll('-', '');
  let stream;
  await adapter.prepareTurn({caseId, marker, configuration});
  try {
    await adapter.releaseTurn({caseId});
    stream = await api.send('a', agentId,
      'Return the exact marker ' + marker + ' without tools or file changes.', caseId);
    const response = await poll(async () => {
      const {body} = await api.rawRequest('b', 'GET', '/api/v2/responses/' + stream.responseId);
      return assertResponse(body, agentId, stream.responseId);
    }, terminal, {timeout: 90000, code: 'configuration_probe_response_timeout'});
    check(response.status === 'completed' && JSON.stringify(response.output ?? []).includes('WS4 complete ' + marker),
      'configuration_probe_response_unbound');
    await stream.done;
    check(stream.error === null && stream.events.filter(event => event.type === 'response.completed').length === 1,
      'configuration_probe_stream_unconfirmed');
    const oracle = await poll(() => adapter.oracle({caseId}), value => value.active_streams === 0,
      {timeout: 20000, code: 'configuration_probe_provider_still_active'});
    await poll(async () => (await api.rawRequest('b', 'GET', '/api/v1/agents/' + agentId)).body,
      value => value?.id === agentId && value.status === 'ready',
      {timeout: 20000, code: 'configuration_probe_agent_not_idle'});
    assertOracle(oracle, caseId, 1);
    check(oracle.configuration_matches === 1, 'native_configuration_mismatch');
  } finally {
    if (stream) { stream.abort(); await stream.done; }
  }
}
