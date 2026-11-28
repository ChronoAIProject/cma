import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOracle, assertResponse, check, poll, terminal} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const SUGGESTION = 'Explain the fixture result in one sentence.';

export async function suggestionsCase({api, adapter, record, recordState, recordFailure}, changeTitle) {
  const {agentId} = record;
  const root = '/api/v1/agents/' + agentId;
  const caseId = 'suggestions-' + randomUUID();
  const marker = 'WS4_SUGGEST_' + randomUUID().replaceAll('-', '');
  const prompt = 'Return the exact marker ' + marker + ' without tools or file changes.';
  const answer = 'WS4 complete ' + marker;
  const preferences = root + '/preferences';
  const read = async (am, path) => (await api.rawRequest(am, 'GET', path)).body;
  const rename = async (expected_revision, title) => (await api.request('a', 'PATCH', preferences,
    {body: {expected_revision, title}, key: randomUUID(), timeout: 8000})).body;
  const changed = 'Suggestion fence ' + randomUUID();
  let baseline, stream, witness;
  await adapter.prepareTurn({caseId, marker, suggestions: true});
  await withRestoration(async () => {
    await adapter.releaseTurn({caseId});
    stream = await api.send('a', agentId, prompt, caseId);
    const response = await poll(async () => {
      const body = await read('b', '/api/v2/responses/' + stream.responseId);
      return assertResponse(body, agentId, stream.responseId);
    }, terminal, {timeout: 90000, code: 'suggestion_turn_timeout'});
    check(response.status === 'completed' && JSON.stringify(response.output ?? []).includes(answer),
      'suggestion_turn_completion_unbound');
    const transcript = await read('a', root + '/transcript');
    const turns = transcript?.turns?.filter(turn => turn.owner_message === prompt);
    check(transcript.agent_id === agentId && turns?.length === 1, 'suggestion_transcript_turn_unbound');
    const [turn] = turns, message = turn.messages.at(-1);
    check(turn.outcome?.type === 'completed' && message?.role === 'assistant' && message.content === answer
      && Number.isSafeInteger(turn.turn_sequence) && Number.isSafeInteger(message.sequence)
      && turn.turn_sequence > 0 && message.sequence > turn.turn_sequence, 'suggestion_transcript_answer_unbound');
    const scope = {turn_sequence: turn.turn_sequence, answer_sequence: message.sequence};
    const path = root + '/suggestions?' + new URLSearchParams(scope);
    const oracle = await poll(() => adapter.oracle({caseId}), value => {
      check(value?.case_id === caseId && value.suggestion_requests <= 1 && value.suggestion_responses === 0
        && value.suggestion_released === false, 'suggestion_generation_barrier_changed');
      return value.suggestion_requests === 1 && value.active_streams === 0;
    }, {timeout: 30000, interval: 250, code: 'suggestion_generation_missing'});
    assertOracle(oracle, caseId, 1);
    for (const am of ['a', 'b']) {
      const pending = await read(am, path);
      check(pending.status === 'running' && pending.set === null && pending.observation === null,
        'held_suggestion_published_early');
    }
    if (changeTitle) {
      baseline = await read('a', preferences);
      check(typeof baseline.title === 'string' && baseline.title.length > 0 && baseline.title !== changed
        && Number.isSafeInteger(baseline.revision), 'suggestion_title_baseline_missing');
      const updated = await rename(baseline.revision, changed);
      check(updated.title === changed && updated.revision === baseline.revision + 1, 'suggestion_title_write_unconfirmed');
      const expired = await read('b', path);
      check(expired.status === 'expired' && expired.set === null && expired.observation === null,
        'renamed_agent_kept_stale_suggestion_visible');
    }
    await adapter.releaseTurn({caseId, suggestion: true});
    const settled = await poll(() => read('a', path), value => value.observation !== null,
      {timeout: 20000, interval: 250, code: 'suggestion_publication_not_settled'});
    const {observation, set, status} = settled;
    check(observation?.failure === null && observation.usage?.input_tokens === 1
      && observation.usage?.output_tokens === 1, 'suggestion_generation_result_unconfirmed');
    if (changeTitle) {
      check(status === 'expired' && set === null, 'stale_suggestion_published_after_title_change');
      for (const am of ['a', 'b']) {
        const current = await read(am, preferences);
        check(current.title === changed && current.revision === baseline.revision + 1,
          'suggestion_publication_changed_title');
      }
      const effects = await adapter.titleEffects({agentId, title: changed});
      check(effects?.length === 1 && Number.isSafeInteger(effects[0]) && effects[0] > 0,
        'suggestion_race_title_effect_duplicated_or_missing');
    } else {
      check(status === 'ready' && set?.agentId === agentId && set.viewerId === api.config.owner_id
        && set.answerId === 'event:' + scope.answer_sequence && equal(set.scope, scope)
        && set.source?.kind === 'generated' && typeof set.id === 'string' && set.id.length > 0
        && equal(set.items, [{id: set.id + ':0', rank: 0, text: SUGGESTION}]),
      'published_suggestion_scope_or_content_wrong');
      witness = {scope, text: set.items[0].text};
    }
    for (const am of ['b', 'a']) {
      check(equal(await read(am, path), settled), 'suggestion_peer_replay_changed_result');
    }
    const final = await adapter.oracle({caseId});
    assertOracle(final, caseId, 1);
    check(final.suggestion_released === true && final.suggestion_requests === 1 && final.suggestion_responses === 1,
      'suggestion_generation_replayed');
    await recordState('suggestion_publication', {scope, status, generationCount: final.suggestion_requests});
  }, async () => {
    await withRestoration(async () => {
      try {
        const gates = await Promise.allSettled([
          adapter.releaseTurn({caseId}), adapter.releaseTurn({caseId, suggestion: true}),
        ]);
        const failed = gates.find(gate => gate.status === 'rejected');
        if (failed) throw failed.reason;
      } finally {
        if (stream) { stream.abort(); await stream.done; }
      }
    }, async () => {
      if (!baseline) return;
      const current = await read('a', preferences);
      if (current.title !== baseline.title) {
        check(current.title === changed && current.revision === baseline.revision + 1,
          'suggestion_title_restore_foreign_write');
        await rename(current.revision, baseline.title);
      }
      for (const am of ['a', 'b']) {
        check((await read(am, preferences)).title === baseline.title, 'suggestion_title_restore_unconfirmed');
      }
    }, (stage, error) => recordFailure('restoration', error));
  }, recordFailure);
  return witness;
}
