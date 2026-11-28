import {randomUUID} from 'node:crypto';
import {isDeepStrictEqual as equal} from 'node:util';
import {assertOracle, assertResponse, check, poll, terminal} from '../cma/core.mjs';
import {withRestoration} from '../failure.mjs';

const generated = 'Generated Fixture Conversation';
const preferences = agentId => `/api/v1/agents/${agentId}/preferences`;

async function readCreationProvenance(api, agentId, expected) {
  const path = `/api/v1/agents/${agentId}/provenance`;
  for (const am of ['a', 'b']) {
    const {body: provenance} = await api.rawRequest(am, 'GET', path);
    check(provenance?.agent_id === agentId && provenance.creation_origin === 'manual'
      && Number.isFinite(Date.parse(provenance.created_at)) && provenance.details?.trigger == null
      && provenance.details?.channel_bridge == null, 'title_creation_provenance_unbound');
    expected ??= provenance;
    check(equal(provenance, expected), 'title_creation_provenance_changed');
  }
  return expected;
}

async function confirmCustody(adapter, agentId, earlier) {
  const observed = await adapter.oracle({caseId: earlier.caseId});
  assertOracle(observed, earlier.caseId, 1);
  check(observed.title_requests === 1 && observed.title_responses === 1
    && observed.title_request_sha256 === earlier.digest, 'title_model_effect_changed_or_repeated');
  const intent = await adapter.titleIntent({agentId});
  check(intent?.request_sha256 === earlier.digest && intent.turn_sequence === earlier.turnSequence
    && intent.accepted === true,
    'title_effect_custody_changed');
}

async function confirmOwnerTitle(api, agentId, title, minimumRevision) {
  let revision;
  for (const am of ['a', 'b']) {
    const {body: current} = await api.rawRequest(am, 'GET', preferences(agentId));
    const {body: detail} = await api.rawRequest(am, 'GET', `/api/v1/agents/${agentId}`);
    revision ??= current?.revision;
    check(current?.title === title && Number.isSafeInteger(revision) && revision >= minimumRevision
      && current.revision === revision
      && detail?.id === agentId && detail.owner_title === title, 'owner_title_readback_changed');
  }
}

export async function generatedTitleCustody({adapter, api, record, recordFailure}) {
  const {agentId} = record;
  const caseId = 'title-' + randomUUID();
  const marker = 'WS4_TITLE_' + randomUUID().replaceAll('-', '');
  const path = preferences(agentId);
  const baseline = (await api.request('a', 'GET', path)).body;
  const provenance = await readCreationProvenance(api, agentId);
  const newTitle = 'Owner Selected Fixture ' + randomUUID().slice(0, 8);
  check(Number.isSafeInteger(baseline.revision) && baseline.title !== newTitle, 'title_fixture_baseline_unbound');
  let stream;
  await adapter.prepareTurn({caseId, marker, title: true});
  await withRestoration(async () => {
    await adapter.releaseTurn({caseId});
    stream = await api.send('a', agentId, 'Return ' + marker + ' without tools or file changes.', caseId);
    const held = await poll(() => adapter.oracle({caseId}), value => {
      check(value?.case_id === caseId && value.title_requests <= 1 && value.title_responses === 0,
        'title_generation_barrier_changed');
      return value.title_requests === 1;
    }, {timeout: 30000, interval: 100, code: 'title_generation_request_missing'});
    check(/^[a-f0-9]{64}$/.test(held.title_request_sha256 ?? ''), 'title_provider_request_digest_missing');
    const renamed = (await api.request('a', 'PATCH', path, {key: randomUUID(),
      body: {expected_revision: baseline.revision, title: newTitle}, timeout: 5000})).body;
    check(renamed.title === newTitle && renamed.revision === baseline.revision + 1, 'held_title_rename_not_confirmed');
    await readCreationProvenance(api, agentId, provenance);
    await adapter.releaseTurn({caseId, title: true});
    await poll(() => adapter.oracle({caseId}), value => value.title_responses === 1,
      {timeout: 10000, interval: 100, code: 'title_generation_response_missing'});
    const response = await poll(async () => assertResponse((await api.rawRequest('b', 'GET',
      '/api/v2/responses/' + stream.responseId)).body, agentId, stream.responseId), terminal,
    {timeout: 20000, code: 'title_trigger_turn_did_not_settle'});
    check(response.status === 'completed', 'title_trigger_turn_failed');
    const transcript = (await api.rawRequest('b', 'GET', `/api/v1/agents/${agentId}/transcript`)).body;
    const touched = transcript.turns?.filter(turn => turn.owner_message?.includes(marker));
    check(transcript.agent_id === agentId && touched?.length === 1
      && Number.isSafeInteger(touched[0].turn_sequence), 'title_trigger_turn_unbound');
    const generatedEffects = await poll(() => adapter.titleEffects({agentId, title: generated}), value => {
      check(Array.isArray(value) && value.length <= 1, 'generated_title_effect_repeated');
      return value.length === 1;
    }, {timeout: 15000, code: 'generated_title_publication_missing'});
    const ownerEffects = await adapter.titleEffects({agentId, title: newTitle});
    check(ownerEffects?.length === 1 && Number.isSafeInteger(ownerEffects[0]) && ownerEffects[0] > 0,
      'owner_title_effect_missing_or_repeated');
    // TitleSet records the generated title and owner override as independent fields.
    check(Number.isSafeInteger(generatedEffects[0]) && generatedEffects[0] > ownerEffects[0],
      'title_publication_order_unbound');
    const proof = {caseId, digest: held.title_request_sha256, turnSequence: touched[0].turn_sequence,
      generatedEffect: generatedEffects[0]};
    await confirmCustody(adapter, agentId, proof);
    await confirmOwnerTitle(api, agentId, newTitle, renamed.revision);
    await readCreationProvenance(api, agentId, provenance);
    record.generatedTitleProof = {...proof, ownerTitle: newTitle, ownerRevision: renamed.revision, ownerEffect: ownerEffects[0]};
    record.generatedTitleProvenance = provenance;
  }, async () => {
    try { await adapter.releaseTurn({caseId, title: true}); }
    finally { if (stream) { stream.abort(); await stream.done; } }
  }, recordFailure);
}

export async function titleCustodyAfterRestart({adapter, api, record}) {
  check(record.generatedTitleProof && record.generatedTitleProvenance, 'pre_restart_title_evidence_missing');
  const proof = record.generatedTitleProof;
  await confirmCustody(adapter, record.agentId, proof);
  const generatedEffects = await adapter.titleEffects({agentId: record.agentId, title: generated});
  check(equal(generatedEffects, [proof.generatedEffect]), 'generated_title_effect_changed_after_restart');
  const effects = await adapter.titleEffects({agentId: record.agentId, title: proof.ownerTitle});
  check(equal(effects, [proof.ownerEffect]), 'owner_title_effect_changed_after_restart');
  await readCreationProvenance(api, record.agentId, record.generatedTitleProvenance);
  await confirmOwnerTitle(api, record.agentId, record.writerTitle?.title ?? proof.ownerTitle,
    record.writerTitle?.revision ?? proof.ownerRevision);
}
