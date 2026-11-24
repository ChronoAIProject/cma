// Offline trace contract only. The caller owns authenticated candidate evidence
// collection; this tool neither contacts a service nor authorizes activation.
import { createHash } from 'node:crypto';
import { closeSync, constants, fstatSync, openSync, readFileSync, readSync, statSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const RELEASE = Object.freeze([
  'register_body_attempt', 'read_source_attempt', 'final_check_attempt',
  'chunk_0_handoff_attempt', 'chunk_1_handoff_attempt', 'close_body',
]);
const REVOKE = Object.freeze([
  'revoke_request', 'freeze_commit', 'deliver_barrier', 'close_release_gate',
  'durable_release_ack', 'effective_commit', 'final_response',
]);
const ROUTES = new Set([
  'shared_history_projection', 'artifact_download', 'source_read', 'retained_output',
  'document_source', 'responses_v2_buffered', 'responses_v2_paged',
]);
const BINDING_KEYS = [
  'source_revision', 'api_image_sha256', 'runtime_image_sha256',
  'web_bundle_sha256', 'configuration_sha256', 'observer_sha256', 'route',
];
const HASH = /^[a-f0-9]{64}$/;
const integer = value => Number.isSafeInteger(value) && value >= 0;
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const exactKeys = (value, names) => object(value) &&
  Object.keys(value).sort().join(',') === [...names].sort().join(',');

export const releaseRoutes = () => [...ROUTES];

export function releaseSchedules() {
  const result = [];
  function visit(left, right, actions) {
    if (left === RELEASE.length && right === REVOKE.length) {
      result.push({
        id: createHash('sha256').update(JSON.stringify(actions)).digest('hex'),
        actions,
      });
      return;
    }
    if (left < RELEASE.length) visit(left + 1, right, [...actions, RELEASE[left]]);
    if (right < REVOKE.length) visit(left, right + 1, [...actions, REVOKE[right]]);
  }
  visit(0, 0, []);
  return result;
}

export function validBinding(binding) {
  return exactKeys(binding, BINDING_KEYS) &&
    typeof binding.source_revision === 'string' && /^[a-f0-9]{40}$/.test(binding.source_revision) &&
    BINDING_KEYS.filter(key => key.endsWith('_sha256')).every(key =>
      typeof binding[key] === 'string' && HASH.test(binding[key])) && ROUTES.has(binding.route);
}

function sameBinding(actual, expected) {
  return validBinding(actual) && BINDING_KEYS.every(key => actual[key] === expected[key]);
}

export function evaluateReleaseTrace(schedule, observation, binding) {
  const result = { schedule_id: schedule.id, origin: 'unknown', status: 'blocked', reasons: [] };
  const blocked = code => ({ ...result, reasons: [code] });
  if (!validBinding(binding)) return blocked('invalid_expected_binding');
  if (!object(observation) || observation.schema !== 1 || observation.schedule_id !== schedule.id)
    return blocked('missing_or_wrong_schedule');
  if (!['candidate', 'synthetic'].includes(observation.origin)) return blocked('unknown_origin');
  result.origin = observation.origin;
  if (!sameBinding(observation.binding, binding)) return blocked('candidate_binding_mismatch');
  if (observation.execution !== 'observed') return blocked('schedule_not_executed');
  if (observation.boundary !== 'server_transport_handoff') return blocked('wrong_byte_boundary');
  if (!exactKeys(observation.identity, ['operation', 'grant', 'release', 'api_boot']) ||
      Object.values(observation.identity).some(value => typeof value !== 'string' || !HASH.test(value)))
    return blocked('missing_retained_identity');
  const actions = [...schedule.actions, 'observation_complete'];
  if (!Array.isArray(observation.events) || observation.events.length !== actions.length)
    return blocked('incomplete_observation_window');
  const events = observation.events;
  for (let index = 0; index < events.length; index++) {
    const event = events[index];
    if (!object(event) || event.action !== actions[index] || event.sequence !== index + 1 ||
        !integer(event.elapsed_ms) || !integer(event.private_bytes_total) ||
        !integer(event.owner_bytes_total) || !integer(event.buffered_private_bytes))
      return blocked('invalid_event');
    if (index && (event.elapsed_ms < events[index - 1].elapsed_ms ||
        event.private_bytes_total < events[index - 1].private_bytes_total ||
        event.owner_bytes_total < events[index - 1].owner_bytes_total))
      return blocked('nonmonotonic_observation');
    if (['durable_release_ack', 'effective_commit'].includes(event.action) &&
        (typeof event.receipt_sha256 !== 'string' || !HASH.test(event.receipt_sha256)))
      return blocked('missing_durable_receipt');
  }
  if (events[0].private_bytes_total !== 0 || events[0].owner_bytes_total !== 0)
    return blocked('missing_zero_counter_baseline');
  const at = action => events.findIndex(event => event.action === action);
  const effective = at('effective_commit');
  const chunks = [at('chunk_0_handoff_attempt'), at('chunk_1_handoff_attempt')];
  const sourceBytes = events[at('read_source_attempt')].source_private_bytes;
  const authorized = events[at('final_check_attempt')].authorized;
  if (!integer(sourceBytes) || typeof authorized !== 'boolean')
    return blocked('missing_source_or_authority_observation');
  const gate = Math.min(at('close_release_gate'), at('close_body'));
  const registrationRefused = at('register_body_attempt') > at('freeze_commit');
  const failures = new Set();
  for (let index = 0; index < events.length; index++) {
    const event = events[index];
    const delta = event.private_bytes_total - (events[index - 1]?.private_bytes_total ?? 0);
    if (index < chunks[0] && delta > 0) failures.add('private_bytes_before_first_handoff');
    if (index > effective && delta > 0) failures.add('private_bytes_after_effective');
    if (index > gate && delta > 0) failures.add('private_bytes_after_local_close');
    if (registrationRefused && event.private_bytes_total > 0)
      failures.add('private_bytes_from_refused_registration');
    if (!authorized && event.private_bytes_total > 0)
      failures.add('private_bytes_from_refused_final_check');
    if (index >= at('durable_release_ack') && event.buffered_private_bytes > 0)
      failures.add('private_buffer_survives_ack');
  }
  const last = events.at(-1);
  result.final_check_authorized = authorized;
  result.counters = {
    private_bytes_total: last.private_bytes_total,
    private_bytes_after_effective: last.private_bytes_total - events[effective].private_bytes_total,
    owner_bytes_after_effective: last.owner_bytes_total - events[effective].owner_bytes_total,
    source_private_bytes: sourceBytes,
  };
  if (failures.size) return { ...result, status: 'fail', reasons: [...failures] };
  // Each sample includes counters attributed at the actual transport handoff.
  // Async sends may become visible at later action samples, not just chunk steps.
  // Aggregate bytes alone cannot establish which chunk was handed off.
  for (let index = 0; index < events.length; index++) {
    const counts = events[index].private_bytes_by_chunk;
    if (!Array.isArray(counts) || counts.length !== 2 || !counts.every(integer))
      return blocked('missing_or_invalid_chunk_counters');
    if (counts[0] + counts[1] !== events[index].private_bytes_total)
      return blocked('chunk_byte_total_mismatch');
    if (index && counts.some((count, chunk) => count < events[index - 1].private_bytes_by_chunk[chunk]))
      return blocked('nonmonotonic_chunk_observation');
    if (counts.some((count, chunk) => count > 0 && index < chunks[chunk]))
      return { ...result, status: 'fail', reasons: ['private_bytes_before_chunk_handoff'] };
  }
  result.counters.first_chunk_bytes_before_effective = events[effective - 1].private_bytes_by_chunk[0];
  if (last.private_bytes_total > 0 && sourceBytes === 0) return blocked('missing_private_source_positive');
  if (result.counters.owner_bytes_after_effective === 0) return blocked('missing_independent_owner_positive');
  const beforeRevoke = at('chunk_1_handoff_attempt') < at('revoke_request');
  if (beforeRevoke && events[at('chunk_1_handoff_attempt')].private_bytes_total === 0)
    return blocked('missing_shared_release_positive');
  return { ...result, status: 'pass' };
}

// Status describes supplied trace contracts, never complete R3 qualification.
// A report cannot erase a failed attempt by supplying a later passing duplicate.
export function checkReleaseCampaign(binding, observations) {
  const schedules = releaseSchedules();
  const byId = new Map(schedules.map(schedule => [schedule.id, schedule]));
  const attempts = [];
  const errors = [];
  const seen = new Set();
  const operations = new Set();
  const releases = new Set();
  if (!validBinding(binding)) errors.push('invalid_expected_binding');
  if (!Array.isArray(observations) || observations.length > schedules.length * 2) {
    observations = [];
    errors.push('invalid_observation_inventory');
  }
  for (const observation of observations) {
    const schedule = object(observation) ? byId.get(observation.schedule_id) : undefined;
    if (!schedule) { errors.push('unknown_schedule'); continue; }
    if (seen.has(schedule.id)) errors.push('duplicate_schedule');
    seen.add(schedule.id);
    const result = evaluateReleaseTrace(schedule, observation, binding);
    if (result.origin === 'candidate' && result.status === 'pass') {
      const { operation, release } = observation.identity;
      if (operations.has(operation) || releases.has(release)) {
        result.status = 'blocked';
        result.reasons.push('reused_retained_identity');
      }
      operations.add(operation);
      releases.add(release);
    }
    attempts.push(result);
  }
  const passed = attempts.filter(row => row.origin === 'candidate' && row.status === 'pass');
  const candidateIds = new Set(passed.map(row => row.schedule_id));
  if (candidateIds.size < 1000) errors.push('fewer_than_1000_distinct_candidate_schedules');
  const passedById = new Map(passed.map(row => [row.schedule_id, row]));
  const covers = predicate => schedules.some(schedule => candidateIds.has(schedule.id) &&
    predicate(schedule.actions, passedById.get(schedule.id)));
  if (!covers(actions => actions.indexOf('chunk_1_handoff_attempt') < actions.indexOf('revoke_request')))
    errors.push('missing_shared_positive_schedule');
  if (!covers((actions, row) => actions.indexOf('final_check_attempt') < actions.indexOf('revoke_request') &&
      actions.indexOf('effective_commit') < actions.indexOf('chunk_0_handoff_attempt') &&
      row.counters.source_private_bytes > 0 && row.final_check_authorized))
    errors.push('missing_pause_before_first_chunk');
  if (!covers((actions, row) => actions.indexOf('chunk_0_handoff_attempt') < actions.indexOf('effective_commit') &&
      actions.indexOf('effective_commit') < actions.indexOf('chunk_1_handoff_attempt') &&
      row.counters.first_chunk_bytes_before_effective > 0))
    errors.push('missing_pause_between_chunks');
  const failed = attempts.filter(row => row.status === 'fail').length;
  const blocked = attempts.filter(row => row.status === 'blocked').length;
  return {
    schema: 1,
    scope: 'release_trace_contract',
    status: failed ? 'fail' : errors.length || blocked ? 'blocked' : 'pass',
    activation_authorized: false,
    qualification: 'not_assessed',
    distinct_candidate_schedules_passed: candidateIds.size,
    synthetic_schedules_passed: attempts.filter(row => row.origin === 'synthetic' && row.status === 'pass').length,
    failed_attempts: failed,
    blocked_attempts: blocked,
    not_observed: schedules.filter(schedule => !seen.has(schedule.id)).map(schedule => schedule.id),
    errors: [...new Set(errors)],
    attempts,
  };
}

function jsonFile(path) {
  if (!path || statSync(path).size > 32 * 1024 * 1024) throw new Error('invalid input');
  return JSON.parse(readFileSync(path, 'utf8'));
}

// Re-evaluate raw traces. A prior report's status or digest is not evidence.
export function checkReleaseMatrix(binding, entries, load = entry => entry.observations) {
  const errors = [];
  const campaigns = [];
  const seen = new Set();
  const releases = new Map();
  if (!object(binding) || Object.hasOwn(binding, 'route') ||
      !validBinding({ ...binding, route: releaseRoutes()[0] }))
    errors.push('invalid_expected_binding');
  if (!Array.isArray(entries)) {
    entries = [];
    errors.push('invalid_route_inventory');
  }
  const maximum = ROUTES.size * 2;
  if (entries.length > maximum) errors.push('oversized_route_inventory');
  for (const [index, entry] of entries.slice(0, maximum).entries()) {
    if (!object(entry) || !ROUTES.has(entry.route)) {
      errors.push('invalid_route_entry');
      campaigns.push({ index, route: null, status: 'blocked', errors: ['invalid_route_entry'] });
      continue;
    }
    const route = entry.route;
    if (seen.has(route)) errors.push('duplicate_route');
    seen.add(route);
    let observations;
    try {
      observations = load(entry);
    } catch {
      campaigns.push({ index, route, status: 'blocked', errors: ['invalid_route_input'] });
      continue;
    }
    const report = checkReleaseCampaign({ ...binding, route }, observations);
    const result = { index, route, status: report.status, errors: [], report };
    if (report.status === 'pass') {
      for (const row of observations) {
        if (row.origin !== 'candidate') continue;
        const previous = releases.get(row.identity.release);
        if (previous && previous !== route) {
          result.status = 'blocked';
          result.errors.push('release_identity_reused_across_routes');
        }
        releases.set(row.identity.release, route);
      }
      result.errors = [...new Set(result.errors)];
    }
    campaigns.push(result);
  }
  const missing = releaseRoutes().filter(route => !seen.has(route));
  if (missing.length) errors.push('missing_routes');
  return {
    schema: 1, scope: 'release_route_matrix_contract',
    status: campaigns.some(row => row.status === 'fail') ? 'fail' :
      errors.length || campaigns.some(row => row.status === 'blocked') ? 'blocked' : 'pass',
    activation_authorized: false, qualification: 'not_assessed',
    required_routes: releaseRoutes(), missing_routes: missing,
    errors: [...new Set(errors)], campaigns,
  };
}

function boundedFile(path, maximum) {
  const fd = openSync(path, constants.O_RDONLY | constants.O_NONBLOCK);
  try {
    const info = fstatSync(fd);
    if (!info.isFile() || info.size > maximum) throw new Error('invalid input');
    const bytes = Buffer.alloc(maximum + 1);
    let used = 0;
    while (used < bytes.length) {
      const size = readSync(fd, bytes, used, bytes.length - used, null);
      if (!size) break;
      used += size;
    }
    if (used > maximum) throw new Error('invalid input');
    return bytes.subarray(0, used);
  } finally {
    closeSync(fd);
  }
}

function matrixFiles(bindingPath, manifestPath) {
  const binding = JSON.parse(boundedFile(bindingPath, 64 * 1024).toString('utf8'));
  const manifest = JSON.parse(boundedFile(manifestPath, 64 * 1024).toString('utf8'));
  const entries = exactKeys(manifest, ['schema', 'routes']) && manifest.schema === 1
    ? manifest.routes : null;
  return checkReleaseMatrix(binding, entries, entry => {
    if (!exactKeys(entry, ['route', 'observations_file', 'observations_sha256']) ||
        typeof entry.observations_file !== 'string' || !entry.observations_file ||
        typeof entry.observations_sha256 !== 'string' || !HASH.test(entry.observations_sha256))
      throw new Error('invalid input');
    const bytes = boundedFile(resolve(dirname(manifestPath), entry.observations_file), 32 * 1024 * 1024);
    if (createHash('sha256').update(bytes).digest('hex') !== entry.observations_sha256)
      throw new Error('invalid input');
    return JSON.parse(bytes.toString('utf8'));
  });
}

export function main(args) {
  if (args.length === 1 && args[0] === 'matrix-plan') {
    return { exit: 2, report: {
      schema: 1, scope: 'release_route_matrix_contract', status: 'blocked',
      activation_authorized: false, qualification: 'not_assessed',
      routes: releaseRoutes().map(route => ({ route, execution: 'not_executed' })),
    } };
  }
  if (args.length === 3 && args[0] === 'check-matrix') {
    const report = matrixFiles(args[1], args[2]);
    return { exit: report.status === 'pass' ? 0 : report.status === 'fail' ? 1 : 2, report };
  }
  if (args.length === 1 && args[0] === 'plan') {
    return { exit: 2, report: {
      schema: 1, scope: 'release_trace_contract', status: 'blocked',
      activation_authorized: false, qualification: 'not_assessed',
      schedules: releaseSchedules().map(schedule => ({ ...schedule, execution: 'not_executed' })),
    } };
  }
  if (args.length !== 3 || args[0] !== 'check') throw new Error('invalid arguments');
  const report = checkReleaseCampaign(jsonFile(args[1]), jsonFile(args[2]));
  return { exit: report.status === 'pass' ? 0 : report.status === 'fail' ? 1 : 2, report };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const result = main(process.argv.slice(2));
    process.stdout.write(JSON.stringify(result.report) + '\n');
    process.exitCode = result.exit;
  } catch {
    process.stdout.write(JSON.stringify({ status: 'blocked', errors: ['invalid_input'], activation_authorized: false, qualification: 'not_assessed' }) + '\n');
    process.exitCode = 2;
  }
}
