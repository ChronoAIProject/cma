import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {planEffectRecovery, recoveryIntentDigest, MAX_RECOVERY_EVENTS} from './effect-recovery.mjs';

const hash = value => createHash('sha256').update(value).digest('hex');
const intent = (operation = 'settings_patch', changes = {}) => ({
  schema: 'autopause-effect-v1', campaign_id: 'campaign_fixture', effect_id: 'effect_fixture',
  operation, resource_id: operation === 'settings_patch' ? 'platform' : 'resource_fixture',
  subject_sha256: hash('subject'), candidate_sha256: hash('candidate'),
  fixture_sha256: hash('fixture'), request_sha256: hash('exact request bytes'),
  ...(['settings_patch', 'admin_always_on'].includes(operation)
    ? {expected_revision: '7', original_values_sha256: hash('original'), proposed_values_sha256: hash('proposed')}
    : {key_sha256: hash('original request key')}),
  ...(operation === 'create_agent' ? {resource_id: 'new'} : {}),
  ...(operation === 'sandbox_command' ? {expected_revision: '7'} : {}), ...changes,
});
const history = (value, ...events) => events.map((event, sequence) => ({
  sequence, intent_digest: recoveryIntentDigest(value), ...event,
}));
const dispatch = {type: 'dispatch_recorded'};
const unavailable = (reason = 'transport_failed') => ({type: 'unavailable', reason});
const ack = (receipt, source = 'direct_response') => ({type: 'acknowledged', source, receipt});
const revisionAck = (value, revision = '8') => ack({
  resource_id: value.resource_id, revision, values_sha256: value.proposed_values_sha256,
});
const diagnostic = (value, revision = '8') => ({type: 'revision_read',
  resource_id: value.resource_id, revision, values_sha256: value.proposed_values_sha256});
const plan = (value, ...events) => planEffectRecovery(value, history(value, ...events));

test('CAS recovery requires an acknowledged write and a later matching revision', () => {
  for (const operation of ['settings_patch', 'admin_always_on']) {
    const value = intent(operation);
    assert.equal(plan(value).decision.action, 'RETAIN_CUSTODY');
    assert.equal(plan(value, dispatch).state, 'OUTCOME_UNKNOWN');
    const unknown = plan(value, dispatch, unavailable('audit_failed'), diagnostic(value));
    assert.equal(unknown.state, 'OUTCOME_UNKNOWN');
    assert.equal(unknown.decision.action, 'READ_REVISION');
    assert.equal(plan(value, dispatch, revisionAck(value)).decision.action, 'READ_REVISION');
    const owned = plan(value, dispatch, revisionAck(value), diagnostic(value));
    assert.equal(owned.decision.action, 'PROPOSE_CAS_RESTORE');
    assert.equal(owned.decision.expected_revision, '8');
    assert.equal(owned.decision.restore_values_sha256, value.original_values_sha256);
    assert.equal(owned.mutations_authorized, false);
    assert.equal(owned.cleanup, 'RETAIN_CUSTODY');
  }
});

test('equal values from another writer cannot raise the restoration revision', () => {
  const value = intent();
  const result = plan(value, dispatch, revisionAck(value), diagnostic(value, '9'));
  assert.equal(result.decision.action, 'RETAIN_CUSTODY');
  assert.equal(result.decision.reason, 'revision_changed');
  assert.equal(Object.hasOwn(result.decision, 'expected_revision'), false);
  assert.equal(plan(value, dispatch, revisionAck(value), diagnostic(value, '6')).decision.action,
    'RETAIN_CUSTODY');
});

test('late acknowledgement needs a fresh observation and duplicate acknowledgement is stable', () => {
  const value = intent();
  const events = [dispatch, unavailable(), diagnostic(value), revisionAck(value)];
  assert.equal(plan(value, ...events).decision.action, 'READ_REVISION');
  const confirmed = [...events, diagnostic(value), revisionAck(value)];
  assert.equal(plan(value, ...confirmed).decision.action, 'PROPOSE_CAS_RESTORE');
  assert.equal(plan(value, ...confirmed, unavailable()).decision.action, 'READ_REVISION');
});

test('conflicting receipts and same-revision contradictory values quarantine permanently', () => {
  const value = intent();
  const conflicts = [revisionAck(value, '9'), {...diagnostic(value), values_sha256: hash('foreign')}];
  for (const conflict of conflicts) {
    const result = plan(value, dispatch, revisionAck(value), conflict, diagnostic(value));
    assert.equal(result.state, 'QUARANTINED');
    assert.equal(result.decision.action, 'RETAIN_CUSTODY');
  }
});

test('revision arithmetic preserves values beyond the JavaScript safe integer range', () => {
  const value = intent('settings_patch', {expected_revision: '9007199254740993'});
  const result = plan(value, dispatch, revisionAck(value, '9007199254740994'),
    diagnostic(value, '9007199254740994'));
  assert.equal(result.decision.expected_revision, '9007199254740994');
  for (const revision of ['0', '01', '-1', '1e2', '18446744073709551616', 7, null]) {
    assert.throws(() => recoveryIntentDigest({...value, expected_revision: revision}), /recovery_revision_invalid/);
  }
  assert.throws(() => plan(value, dispatch, revisionAck(value, '9007199254740993')),
    /recovery_ack_revision_invalid/);
});

test('unknown create retains custody; known lifecycle identity only plans a read', () => {
  const value = intent('create_agent');
  assert.equal(plan(value, dispatch, unavailable('not_found')).decision.action, 'RETAIN_CUSTODY');
  const result = plan(value, dispatch, ack({operation_id: 'operation_fixture'}));
  assert.equal(result.decision.action, 'READ_LIFECYCLE');
  assert.equal(result.decision.path, '/api/v1/agent-lifecycle/operation_fixture');
  assert.equal(result.cleanup, 'RETAIN_CUSTODY');
  assert.throws(() => plan(value, dispatch, ack({operation_id: 'operation_fixture'}, 'receipt_lookup')),
    /recovery_ack_source_invalid/);
});

test('Responses lookup is read-only POST with the exact original request binding', () => {
  const value = intent('response_input');
  const result = plan(value, dispatch, unavailable('receipt_missing'));
  assert.equal(result.state, 'OUTCOME_UNKNOWN');
  assert.equal(result.decision.action, 'READ_INPUT_RECEIPT');
  assert.equal(result.decision.method, 'POST');
  assert.equal(result.decision.path, '/api/v2/agents/resource_fixture/responses/receipt');
  assert.equal(result.decision.request_sha256, value.request_sha256);
  assert.equal(result.decision.key_sha256, value.key_sha256);
  const receipt = {command_id: 'command_fixture', agent_id: value.resource_id, response_id: 'response_fixture'};
  for (const source of ['direct_response', 'receipt_lookup']) {
    const admitted = plan(value, dispatch, ack(receipt, source), unavailable('not_found'));
    assert.equal(admitted.state, 'ACKNOWLEDGED');
    assert.deepEqual(admitted.decision.paths,
      ['/api/v2/commands/command_fixture', '/api/v2/responses/response_fixture']);
    assert.equal(admitted.cleanup, 'RETAIN_CUSTODY');
    assert.equal(admitted.qualification, 'NOT_ASSESSED');
  }
  assert.throws(() => plan(value, dispatch, ack({...receipt, agent_id: 'foreign'})), /recovery_resource_mismatch/);
  assert.throws(() => plan(value, dispatch, ack({...receipt, response_id: null})), /recovery_id_invalid/);
  const conflict = plan(value, dispatch, ack(receipt), ack({...receipt, command_id: 'replacement'}));
  assert.equal(conflict.state, 'QUARANTINED');
});

test('Sandbox command acceptance requires the original target and revision but never proves finality', () => {
  const value = intent('sandbox_command');
  const receipt = {command_id: 'command_fixture', sandbox_id: value.resource_id, expected_revision: '7'};
  assert.equal(plan(value, dispatch).decision.action, 'RETAIN_CUSTODY');
  const result = plan(value, dispatch, ack(receipt));
  assert.equal(result.decision.path, '/api/v1/sandbox-commands/command_fixture');
  assert.equal(result.cleanup, 'RETAIN_CUSTODY');
  assert.throws(() => plan(value, dispatch, ack({...receipt, expected_revision: '8'})), /recovery_resource_mismatch/);
  assert.throws(() => plan(value, dispatch, ack({...receipt, sandbox_id: 'foreign'})), /recovery_resource_mismatch/);
});

test('generic stop/delete acknowledgements and expired receipts never permit automatic replay', () => {
  for (const operation of ['agent_stop', 'agent_delete']) {
    const value = intent(operation);
    assert.equal(plan(value, dispatch, unavailable('recovery_required')).decision.action, 'RETAIN_CUSTODY');
    assert.equal(plan(value, dispatch, ack({status: 202})).decision.action, 'READ_AGENT');
    assert.equal(plan(value, dispatch, ack({status: 202}), unavailable('not_found')).cleanup, 'RETAIN_CUSTODY');
    assert.throws(() => plan(value, dispatch, ack({status: 200})), /recovery_ack_status_invalid/);
    assert.throws(() => plan(value, dispatch, ack({status: 202}, 'receipt_lookup')), /recovery_ack_source_invalid/);
  }
});

test('an event cannot be replayed against another owner, request, key, fixture or candidate', () => {
  const value = intent('response_input');
  const events = history(value, dispatch);
  for (const key of ['subject_sha256', 'candidate_sha256', 'fixture_sha256', 'request_sha256', 'key_sha256']) {
    assert.throws(() => planEffectRecovery({...value, [key]: hash('different')}, events), /recovery_event_binding_invalid/);
  }
  for (const key of ['campaign_id', 'effect_id', 'resource_id']) {
    assert.throws(() => planEffectRecovery({...value, [key]: 'different'}, events), /recovery_event_binding_invalid/);
  }
});

test('canonical bindings are independent of key order and snapshots are immutable', () => {
  const value = intent();
  const reordered = Object.fromEntries(Object.entries(value).reverse());
  assert.equal(recoveryIntentDigest(value), recoveryIntentDigest(reordered));
  const events = history(value, dispatch, revisionAck(value), diagnostic(value));
  const before = JSON.stringify({value, events});
  const result = planEffectRecovery(value, events);
  assert.equal(JSON.stringify({value, events}), before);
  assert.throws(() => {result.decision.expected_revision = '9';}, TypeError);
  assert.throws(() => {result.state = 'SAFE';}, TypeError);
  value.resource_id = 'replacement';
  events[1].receipt.revision = '10';
  assert.equal(result.decision.expected_revision, '8');
});

test('journal failure and binding changes retain custody even before a dispatch record', () => {
  const value = intent();
  for (const reason of ['journal_incomplete', 'journal_unwritable', 'binding_changed']) {
    const quarantined = plan(value, {type: 'quarantine', reason});
    assert.equal(quarantined.state, 'QUARANTINED');
    assert.equal(quarantined.decision.reason, reason);
  }
});

test('sparse, reordered, unbounded and premature events refuse with safe error codes', () => {
  const value = intent();
  for (const events of [null, {}, new Array(1), new Array(MAX_RECOVERY_EVENTS + 1)]) {
    assert.throws(() => planEffectRecovery(value, events), /recovery_/);
  }
  for (const event of [revisionAck(value), unavailable(), diagnostic(value)]) {
    assert.throws(() => plan(value, event), /recovery_dispatch_required/);
  }
  assert.throws(() => plan(value, dispatch, dispatch), /recovery_duplicate_dispatch/);
  assert.throws(() => planEffectRecovery(value, [{...history(value, dispatch)[0], sequence: 1}]),
    /recovery_event_sequence_invalid/);
  assert.throws(() => plan(value, dispatch, {type: 'completed', result: 'PASS'}), /recovery_event_type_invalid/);
  assert.throws(() => plan(value, dispatch, unavailable('provider private payload')), /recovery_reason_invalid/);
});

test('unknown intent fields, malformed values and accessor objects refuse without leaking payloads', () => {
  const value = intent();
  const bad = [null, [], true, Object.create(value), {...value, schema: 'future'},
    {...value, operation: '__proto__'}, {...value, operation: {}}, {...value, resource_id: 'other'},
    {...value, private_payload: 'synthetic private content'}, {...value, effect_id: '../foreign'},
    {...value, fixture_sha256: 'not a digest'}, {...value, key_sha256: hash('not a CAS key')},
    {...intent('create_agent'), resource_id: 'replacement'}, {...value, [Symbol('extra')]: 1}];
  for (const malformed of bad) {
    assert.throws(() => planEffectRecovery(malformed, []), error => {
      assert.match(error.message, /^recovery_[a-z_]+$/);
      assert.equal(error.message.includes('private content'), false);
      return true;
    });
  }
  let reads = 0;
  for (const key of ['operation', 'candidate_sha256']) {
    const accessor = {...value};
    Object.defineProperty(accessor, key, {get() {reads++; return value[key];}});
    assert.throws(() => recoveryIntentDigest(accessor), /recovery_/);
  }
  assert.equal(reads, 0);
  const nullPrototype = Object.assign(Object.create(null), value);
  assert.equal(recoveryIntentDigest(nullPrototype), recoveryIntentDigest(value));
});

test('receipt and event schemas reject wrong-kind evidence and unsafe identifiers', () => {
  const value = intent();
  const wrong = [
    ack({...revisionAck(value).receipt, resource_id: 'foreign'}),
    ack({...revisionAck(value).receipt, values_sha256: hash('unrequested values')}),
    ack({...revisionAck(value).receipt, revision: '7'}),
    revisionAck(value, 8), ack({...revisionAck(value).receipt, unexpected: true}),
    ack(null), {...diagnostic(value), resource_id: 'foreign'},
    {type: 'quarantine', reason: 'arbitrary message'},
  ];
  for (const event of wrong) assert.throws(() => plan(value, dispatch, event), /recovery_/);
  assert.throws(() => plan(value, dispatch, ack(revisionAck(value).receipt, 'receipt_lookup')),
    /recovery_ack_source_invalid/);
  assert.throws(() => plan(intent('response_input'), dispatch, diagnostic(value)), /recovery_event_kind_invalid/);
  assert.throws(() => plan(intent('create_agent'), dispatch, ack({operation_id: '../foreign'})), /recovery_id_invalid/);
  for (const type of [null, {}, 7, '__proto__']) {
    assert.throws(() => plan(value, dispatch, {type}), /recovery_event_type_invalid/);
  }
  assert.throws(() => planEffectRecovery(value, history(value, dispatch, null)), /recovery_event_type_invalid/);
  const events = history(value, dispatch);
  events[0].unexpected = true;
  assert.throws(() => planEffectRecovery(value, events), /recovery_fields_invalid/);
});

test('history properties and getters cannot substitute unrecorded facts', () => {
  const value = intent();
  const extra = history(value, dispatch);
  extra.note = 'unrecorded';
  assert.throws(() => planEffectRecovery(value, extra), /recovery_history_invalid/);
  const inherited = history(value, dispatch);
  Object.setPrototypeOf(inherited, Object.create(Array.prototype));
  assert.throws(() => planEffectRecovery(value, inherited), /recovery_history_invalid/);
  let reads = 0;
  const accessor = history(value, dispatch);
  Object.defineProperty(accessor, '0', {get() {reads++; return dispatch;}});
  assert.throws(() => planEffectRecovery(value, accessor), /recovery_history_invalid/);
  const eventAccessor = history(value, dispatch);
  Object.defineProperty(eventAccessor[0], 'type', {get() {reads++; return 'dispatch_recorded';}});
  assert.throws(() => planEffectRecovery(value, eventAccessor), /recovery_event_type_invalid/);
  const receiptAccessor = {...revisionAck(value).receipt};
  Object.defineProperty(receiptAccessor, 'revision', {get() {reads++; return '8';}});
  assert.throws(() => plan(value, dispatch, ack(receiptAccessor)), /recovery_record_invalid/);
  assert.equal(reads, 0);
});

test('a full bounded history preserves original custody and quarantine validates all later facts', () => {
  const value = intent();
  const events = [dispatch, ...Array.from({length: MAX_RECOVERY_EVENTS - 1}, () => unavailable('timeout'))];
  const result = plan(value, ...events);
  assert.equal(result.sequence, MAX_RECOVERY_EVENTS);
  assert.equal(result.state, 'OUTCOME_UNKNOWN');
  const quarantined = history(value, {type: 'quarantine', reason: 'journal_incomplete'}, dispatch);
  quarantined[1].intent_digest = hash('wrong intent');
  assert.throws(() => planEffectRecovery(value, quarantined), /recovery_event_binding_invalid/);
  const repeated = plan(value, dispatch, {type: 'quarantine', reason: 'journal_unwritable'},
    {type: 'quarantine', reason: 'binding_changed'});
  assert.equal(repeated.decision.reason, 'journal_unwritable');
});

test('all supported outcomes remain proposals with no cleanup or qualification authority', () => {
  const operations = ['settings_patch', 'admin_always_on', 'create_agent', 'response_input',
    'sandbox_command', 'agent_stop', 'agent_delete'];
  for (const operation of operations) {
    const value = intent(operation);
    for (const reason of ['timeout', 'transport_failed', 'audit_failed', 'not_found', 'receipt_missing',
      'forbidden', 'recovery_required', 'cas_conflict']) {
      const result = plan(value, dispatch, unavailable(reason));
      assert.equal(result.mutations_authorized, false);
      assert.equal(result.cleanup, 'RETAIN_CUSTODY');
      assert.equal(result.qualification, 'NOT_ASSESSED');
      assert.ok(!['SEND', 'REPLAY', 'CANCEL', 'DELETE'].includes(result.decision.action));
      assert.equal(result.intent_digest, recoveryIntentDigest(value));
    }
  }
  const value = intent('response_input');
  const result = plan(value, dispatch, ack({command_id: 'command', agent_id: value.resource_id, response_id: 'response'}));
  assert.throws(() => result.decision.paths.push('/new'), TypeError);
});
