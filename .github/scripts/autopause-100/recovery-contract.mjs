import {check, hash, isHash} from '../scale-tests/lib/checks.mjs';

const operations = Object.freeze({
  settings_patch: 'revision', admin_always_on: 'revision', create_agent: 'lifecycle',
  response_input: 'response', sandbox_command: 'sandbox', agent_stop: 'rest', agent_delete: 'rest',
});
const common = ['schema', 'campaign_id', 'effect_id', 'operation', 'resource_id',
  'subject_sha256', 'candidate_sha256', 'fixture_sha256', 'request_sha256'];

export function fields(value, keys) {
  check(value !== null && typeof value === 'object' && !Array.isArray(value), 'recovery_record_invalid');
  check([Object.prototype, null].includes(Object.getPrototypeOf(value)), 'recovery_record_invalid');
  const actual = Reflect.ownKeys(value);
  check(actual.length === keys.length && actual.every(key => keys.includes(key)), 'recovery_fields_invalid');
  check(actual.every(key => Object.hasOwn(Object.getOwnPropertyDescriptor(value, key), 'value')),
    'recovery_record_invalid');
}

export function identifier(value) {
  check(typeof value === 'string' && /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/.test(value), 'recovery_id_invalid');
}

export function revision(value) {
  check(typeof value === 'string' && /^[1-9][0-9]{0,19}$/.test(value)
    && BigInt(value) <= 18446744073709551615n, 'recovery_revision_invalid');
}

export function digest(value) {
  check(isHash(value), 'recovery_digest_invalid');
}

export function kindOf(intent) {
  check(Object.hasOwn(operations, intent.operation), 'recovery_operation_invalid');
  return operations[intent.operation];
}

// Only flat, validated metadata enters the binding. Wire bytes and credentials
// belong to separate custody/auth ports, never this planner or its diagnostics.
export function normalizeIntent(value) {
  check(value !== null && typeof value === 'object', 'recovery_record_invalid');
  const operation = Object.getOwnPropertyDescriptor(value, 'operation');
  check(operation && Object.hasOwn(operation, 'value') && typeof operation.value === 'string',
    'recovery_operation_invalid');
  const kind = kindOf({operation: operation.value});
  const extra = kind === 'revision'
    ? ['expected_revision', 'original_values_sha256', 'proposed_values_sha256']
    : ['key_sha256', ...(kind === 'sandbox' ? ['expected_revision'] : [])];
  fields(value, [...common, ...extra]);
  check(value.schema === 'autopause-effect-v1', 'recovery_schema_invalid');
  for (const key of ['campaign_id', 'effect_id', 'resource_id']) identifier(value[key]);
  for (const key of [...common, ...extra].filter(key => key.endsWith('_sha256'))) digest(value[key]);
  if (kind === 'revision' || kind === 'sandbox') revision(value.expected_revision);
  if (value.operation === 'settings_patch') check(value.resource_id === 'platform', 'recovery_resource_mismatch');
  if (kind === 'lifecycle') check(value.resource_id === 'new', 'recovery_resource_mismatch');
  return Object.freeze(Object.fromEntries([...common, ...extra].sort().map(key => [key, value[key]])));
}

export function recoveryIntentDigest(intent) {
  return hash(JSON.stringify(normalizeIntent(intent)));
}

export function receiptFor(intent, event) {
  const kind = kindOf(intent);
  check(event.source === 'direct_response' || (kind === 'response' && event.source === 'receipt_lookup'),
    'recovery_ack_source_invalid');
  const value = event.receipt;
  const keys = {
    revision: ['resource_id', 'revision', 'values_sha256'], lifecycle: ['operation_id'],
    response: ['command_id', 'agent_id', 'response_id'],
    sandbox: ['command_id', 'sandbox_id', 'expected_revision'], rest: ['status'],
  }[kind];
  fields(value, keys);
  for (const key of keys.filter(key => key.endsWith('_id'))) identifier(value[key]);
  if (kind === 'revision') {
    revision(value.revision); digest(value.values_sha256);
    check(value.resource_id === intent.resource_id, 'recovery_resource_mismatch');
    check(BigInt(value.revision) > BigInt(intent.expected_revision), 'recovery_ack_revision_invalid');
    check(value.values_sha256 === intent.proposed_values_sha256, 'recovery_ack_values_mismatch');
  }
  if (kind === 'response') check(value.agent_id === intent.resource_id, 'recovery_resource_mismatch');
  if (kind === 'sandbox') {
    revision(value.expected_revision);
    check(value.sandbox_id === intent.resource_id && value.expected_revision === intent.expected_revision,
      'recovery_resource_mismatch');
  }
  if (kind === 'rest') check(value.status === 202, 'recovery_ack_status_invalid');
  return Object.freeze(Object.fromEntries(keys.map(key => [key, value[key]])));
}

export function validateEvent(intent, event, sequence, binding) {
  check(event !== null && typeof event === 'object', 'recovery_record_invalid');
  const type = Object.getOwnPropertyDescriptor(event, 'type');
  check(type && Object.hasOwn(type, 'value') && typeof type.value === 'string', 'recovery_event_type_invalid');
  const extra = {
    dispatch_recorded: [], unavailable: ['reason'], acknowledged: ['source', 'receipt'],
    revision_read: ['resource_id', 'revision', 'values_sha256'], quarantine: ['reason'],
  };
  check(Object.hasOwn(extra, type.value), 'recovery_event_type_invalid');
  fields(event, ['type', 'sequence', 'intent_digest', ...extra[type.value]]);
  check(event.sequence === sequence, 'recovery_event_sequence_invalid');
  check(event.intent_digest === binding, 'recovery_event_binding_invalid');
  if (event.type === 'unavailable') {
    check(['transport_failed', 'audit_failed', 'timeout', 'not_found', 'receipt_missing',
      'forbidden', 'recovery_required', 'cas_conflict'].includes(event.reason), 'recovery_reason_invalid');
  }
  if (event.type === 'quarantine') {
    check(['journal_incomplete', 'journal_unwritable', 'binding_changed'].includes(event.reason),
      'recovery_reason_invalid');
  }
  if (event.type === 'acknowledged') receiptFor(intent, event);
  if (event.type === 'revision_read') {
    check(kindOf(intent) === 'revision', 'recovery_event_kind_invalid');
    identifier(event.resource_id); revision(event.revision); digest(event.values_sha256);
    check(event.resource_id === intent.resource_id, 'recovery_resource_mismatch');
  }
}
