import {check, hash} from '../scale-tests/lib/checks.mjs';
import {fields, identifier, digest, kindOf, normalizeIntent} from './recovery-contract.mjs';
import {planEffectRecovery, recoveryIntentDigest} from './effect-recovery.mjs';

export const CUSTODY_SCHEMA = 'autopause-custody-v1';
export const CUSTODY_LIMITS = Object.freeze({records: 512, journalBytes: 1024 * 1024,
  recordBytes: 64 * 1024, effects: 32, contentBytes: 256 * 1024, totalContentBytes: 2 * 1024 * 1024});

export function limitsFor(overrides = {}) {
  check(overrides && typeof overrides === 'object' && !Array.isArray(overrides), 'custody_limits_invalid');
  fields(overrides, Object.keys(overrides));
  const value = {...CUSTODY_LIMITS, ...overrides};
  fields(value, Object.keys(CUSTODY_LIMITS));
  for (const [key, maximum] of Object.entries(CUSTODY_LIMITS)) {
    check(Number.isSafeInteger(value[key]) && value[key] > 0 && value[key] <= maximum, 'custody_limits_invalid');
  }
  return Object.freeze(value);
}

export function bindingFor(value) {
  fields(value, ['campaign_id', 'candidate_sha256', 'fixture_sha256']);
  identifier(value.campaign_id); digest(value.candidate_sha256); digest(value.fixture_sha256);
  return Object.freeze({...value});
}

export function contentRoles(intent) {
  return kindOf(intent) === 'revision'
    ? {body: 'request_sha256', original: 'original_values_sha256', proposed: 'proposed_values_sha256'}
    : {body: 'request_sha256', key: 'key_sha256'};
}

export function contentFor(intent, bytes, limits) {
  const roles = contentRoles(intent);
  fields(bytes, Object.keys(roles));
  return Object.entries(roles).map(([role, digestKey]) => {
    const value = bytes[role];
    check(value instanceof Uint8Array && value.byteLength <= limits.contentBytes, 'custody_content_bound');
    const copy = Buffer.from(value);
    if (role === 'key') check(copy.length >= 1 && copy.length <= 255
      && copy.every(byte => byte >= 0x21 && byte <= 0x7e), 'custody_key_invalid');
    check(hash(copy) === intent[digestKey], 'custody_content_digest_invalid');
    return {bytes: copy, ref: {role, name: intent.effect_id + '.' + role,
      bytes: copy.length, sha256: intent[digestKey]}};
  });
}

export function referencesFor(intent, refs, limits) {
  const roles = Object.entries(contentRoles(intent));
  check(Array.isArray(refs) && refs.length === roles.length, 'custody_content_invalid');
  for (let index = 0; index < roles.length; index++) {
    const [role, digestKey] = roles[index]; const ref = refs[index];
    fields(ref, ['role', 'name', 'bytes', 'sha256']);
    check(ref.role === role && ref.name === intent.effect_id + '.' + role
      && ref.sha256 === intent[digestKey], 'custody_content_invalid');
    check(Number.isSafeInteger(ref.bytes) && ref.bytes >= 0 && ref.bytes <= limits.contentBytes,
      'custody_content_bound');
    if (role === 'key') check(ref.bytes >= 1 && ref.bytes <= 255, 'custody_key_invalid');
  }
}

export function newLedger(binding, limits) {
  return {binding: bindingFor(binding), limits: limitsFor(limits), effects: new Map(), contentBytes: 0};
}

export function applyPayload(ledger, payload) {
  check(payload && typeof payload === 'object', 'custody_payload_invalid');
  if (payload.type === 'intent') {
    fields(payload, ['type', 'intent', 'content']);
    const intent = normalizeIntent(payload.intent);
    check(Object.entries(ledger.binding).every(([key, value]) => intent[key] === value), 'custody_binding_mismatch');
    check(!ledger.effects.has(intent.effect_id), 'custody_duplicate_intent');
    check(ledger.effects.size < ledger.limits.effects, 'custody_capacity_exhausted');
    referencesFor(intent, payload.content, ledger.limits);
    const size = payload.content.reduce((total, ref) => total + ref.bytes, 0);
    check(ledger.contentBytes + size <= ledger.limits.totalContentBytes, 'custody_capacity_exhausted');
    ledger.contentBytes += size;
    ledger.effects.set(intent.effect_id, {intent, content: payload.content, ready: false, events: []});
    return;
  }
  check(['ready', 'event'].includes(payload.type), 'custody_payload_invalid');
  fields(payload, payload.type === 'ready' ? ['type', 'effect_id'] : ['type', 'effect_id', 'event']);
  const effect = ledger.effects.get(payload.effect_id);
  check(effect, 'custody_effect_missing');
  if (payload.type === 'ready') {
    check(!effect.ready && effect.events.length === 0, 'custody_duplicate_ready');
    effect.ready = true;
  } else {
    check(effect.ready, 'custody_intent_not_ready');
    planEffectRecovery(effect.intent, [...effect.events, payload.event]);
    effect.events.push(payload.event);
  }
}

export function eventFor(effect, fact) {
  check(fact && typeof fact === 'object', 'custody_fact_invalid');
  fields(fact, Object.keys(fact));
  check(!Object.hasOwn(fact, 'sequence') && !Object.hasOwn(fact, 'intent_digest'), 'custody_fact_binding_invalid');
  // Validation precedes copying, so accessors cannot become persisted facts.
  const descriptors = Object.getOwnPropertyDescriptors(fact);
  check(Reflect.ownKeys(descriptors).every(key => Object.hasOwn(descriptors[key], 'value')), 'custody_fact_invalid');
  const event = {...fact, sequence: effect.events.length, intent_digest: recoveryIntentDigest(effect.intent)};
  planEffectRecovery(effect.intent, [...effect.events, event]);
  return structuredClone(event);
}

export function encodeRecord(sequence, previous, payload) {
  const record = {schema: CUSTODY_SCHEMA, sequence, previous, payload};
  const sha256 = hash(JSON.stringify(record));
  return {sha256, line: JSON.stringify({...record, sha256}) + '\n'};
}

export function decodeRecord(line, sequence, previous) {
  let record;
  try {record = JSON.parse(line);} catch {throw Error('custody_record_invalid');}
  fields(record, ['schema', 'sequence', 'previous', 'payload', 'sha256']);
  check(record.schema === CUSTODY_SCHEMA && record.sequence === sequence && record.previous === previous,
    'custody_record_binding_invalid');
  const expected = encodeRecord(sequence, previous, record.payload);
  check(record.sha256 === expected.sha256 && line + '\n' === expected.line, 'custody_record_digest_invalid');
  return record;
}
