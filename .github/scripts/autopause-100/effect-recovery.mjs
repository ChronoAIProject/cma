import {check} from '../scale-tests/lib/checks.mjs';
import {kindOf, normalizeIntent, receiptFor, recoveryIntentDigest, validateEvent} from './recovery-contract.mjs';

export {recoveryIntentDigest} from './recovery-contract.mjs';
export const MAX_RECOVERY_EVENTS = 256;

function fold(intent, events, binding) {
  let dispatched = false; let receipt = null; let observation = null; let quarantine = null;
  for (let index = 0; index < events.length; index++) {
    const event = events[index];
    validateEvent(intent, event, index, binding);
    // A complete history is still validated after quarantine; no later event
    // may erase the first corruption or permit a new dispatch.
    if (quarantine) continue;
    if (event.type === 'quarantine') {quarantine = event.reason; continue;}
    if (event.type === 'dispatch_recorded') {
      check(!dispatched, 'recovery_duplicate_dispatch');
      dispatched = true;
      continue;
    }
    check(dispatched, 'recovery_dispatch_required');
    if (event.type === 'unavailable') {observation = null; continue;}
    if (event.type === 'acknowledged') {
      const next = receiptFor(intent, event);
      if (receipt && JSON.stringify(receipt) !== JSON.stringify(next)) {
        quarantine = 'conflicting_acknowledgement';
      } else if (!receipt) {
        receipt = next;
        // A late ACK cannot turn an earlier diagnostic read into a current CAS check.
        observation = null;
      }
      continue;
    }
    observation = event;
    if (receipt && receipt.revision === event.revision && receipt.values_sha256 !== event.values_sha256) {
      quarantine = 'conflicting_revision_observation';
    }
  }
  return {dispatched, receipt, observation, quarantine};
}

const retain = reason => ({action: 'RETAIN_CUSTODY', reason});

function revisionDecision(intent, receipt, observation) {
  const path = intent.operation === 'settings_patch'
    ? '/api/v1/admin/settings' : `/api/v1/admin/sandboxes/${intent.resource_id}`;
  if (!receipt || !observation) return {action: 'READ_REVISION', method: 'GET', path,
    reason: receipt ? 'confirm_owned_revision' : 'revision_outcome_unknown'};
  if (observation.revision !== receipt.revision) return retain('revision_changed');
  // A proposal is not mutation authority. The executor still needs durable
  // custody, current authorization and a new CAS intent at this exact revision.
  return {action: 'PROPOSE_CAS_RESTORE', method: intent.operation === 'settings_patch' ? 'PATCH' : 'POST',
    path: intent.operation === 'settings_patch' ? path : path + '/always-on',
    expected_revision: receipt.revision, restore_values_sha256: intent.original_values_sha256,
    reason: 'owned_revision_observed'};
}

function decision(intent, state) {
  const {dispatched, receipt, observation, quarantine} = state;
  if (quarantine) return retain(quarantine);
  if (!dispatched) return retain('dispatch_not_proven');
  const kind = kindOf(intent);
  if (kind === 'revision') return revisionDecision(intent, receipt, observation);
  if (kind === 'response') {
    if (!receipt) return {action: 'READ_INPUT_RECEIPT', method: 'POST',
      path: `/api/v2/agents/${intent.resource_id}/responses/receipt`,
      request_sha256: intent.request_sha256, key_sha256: intent.key_sha256,
      reason: 'original_input_outcome_unknown'};
    return {action: 'READ_COMMAND_AND_RESPONSE', method: 'GET',
      paths: Object.freeze([`/api/v2/commands/${receipt.command_id}`, `/api/v2/responses/${receipt.response_id}`]),
      reason: 'execution_and_terminal_unproved'};
  }
  if (!receipt) return retain('mutation_replay_not_authorized');
  if (kind === 'lifecycle') return {action: 'READ_LIFECYCLE', method: 'GET',
    path: `/api/v1/agent-lifecycle/${receipt.operation_id}`, reason: 'lifecycle_completion_unproved'};
  if (kind === 'sandbox') return {action: 'READ_SANDBOX_COMMAND', method: 'GET',
    path: `/api/v1/sandbox-commands/${receipt.command_id}`, reason: 'runtime_finality_unproved'};
  return {action: 'READ_AGENT', method: 'GET', path: `/api/v1/agents/${intent.resource_id}`,
    reason: 'runtime_finality_unproved'};
}

/**
 * Plan recovery from an original fixture intent and ordered, independently
 * authenticated journal facts. This function does not verify disk durability,
 * network provenance or completeness of the supplied history. Missing history
 * must be reported as quarantine by its reader, never silently truncated.
 *
 * Intent schema autopause-effect-v1 binds campaign/effect/resource IDs, operation,
 * subject/candidate/fixture/request SHA-256 and either original key SHA-256 or
 * expected revision plus original/proposed writable-value SHA-256. Revisions
 * are positive u64 decimal strings; hashes exclude credentials and private content.
 * Events carry consecutive sequence, intent_digest and a typed fact. Only a
 * direct response may acknowledge CAS; matching GET values cannot do so. The
 * values digest covers writable target fields, excluding server-managed grace.
 *
 * No result authorizes execution, resend, cancellation, deletion or qualification.
 * A restore proposal uses a confirmed revision, never a later observed revision.
 * ACKs plan further reads; they do not prove work terminal or cleanup complete.
 * Invalid input throws a fixed, payload-free GateError. Preserve custody on error.
 */
export function planEffectRecovery(value, events) {
  const intent = normalizeIntent(value);
  check(Array.isArray(events) && events.length <= MAX_RECOVERY_EVENTS, 'recovery_history_invalid');
  check(Object.getPrototypeOf(events) === Array.prototype && Reflect.ownKeys(events).length === events.length + 1,
    'recovery_history_invalid');
  for (let index = 0; index < events.length; index++) {
    const descriptor = Object.getOwnPropertyDescriptor(events, String(index));
    check(descriptor && Object.hasOwn(descriptor, 'value'), 'recovery_history_invalid');
  }
  const binding = recoveryIntentDigest(intent);
  const state = fold(intent, events, binding);
  const status = state.quarantine ? 'QUARANTINED' : state.receipt ? 'ACKNOWLEDGED'
    : state.dispatched ? 'OUTCOME_UNKNOWN' : 'NO_DISPATCH_RECORDED';
  return Object.freeze({scope: 'RECOVERY_PLAN', qualification: 'NOT_ASSESSED',
    intent_digest: binding, sequence: events.length, state: status,
    decision: Object.freeze(decision(intent, state)), mutations_authorized: false, cleanup: 'RETAIN_CUSTODY'});
}
