// Invoked by the sole campaign parent. This module starts no applications,
// browsers, processes, restarts or HPA changes.
import {createHash} from 'node:crypto';
import {loadRegistry} from './registry.mjs';
import {executeCase, Blocked} from './runner.mjs';
import {coverageCounts} from './counts.mjs';
import {fixturesForCase} from './setups.mjs';
import {bindParentFixtures} from './parent-bindings.mjs';

const check = (condition, code) => { if (!condition) throw Error(code); };
const hash = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');

export function validateTopology(topology, image) {
  check(topology?.shared_database === true && topology.coordination_enabled === true,
    'coverage_shared_database_required');
  const {a, b} = topology.ams ?? {};
  for (const member of [a, b]) check(member?.ready === true && member.image === image
    && typeof member.pod_uid === 'string' && member.pod_uid.length > 0
    && typeof member.instance_id === 'string' && member.instance_id.length > 0,
  'coverage_two_ready_exact_image_required');
  check(a.pod_uid !== b.pod_uid && a.instance_id !== b.instance_id, 'coverage_distinct_processes_required');
  return topology;
}

// Observe the witness on both sides of the parent's actual restart. A boolean
// declaration alone does not establish owner loss, takeover or process replacement.
export function verifyRestart(before, after, boundary) {
  if (!before || !after || !boundary) return null;
  const left = before.owner, right = after.owner;
  const joined = boundary.handoffs?.find(row => row.agent_id === left?.agent_id);
  if (boundary.status === 'PASS' && boundary.cleanup?.predecessor_uids_absent === true
    && Array.isArray(boundary.predecessor_uids) && boundary.predecessor_uids.includes(left?.pod_uid)
    && joined?.pass === true && joined.before?.instance_id === left.instance_id
    && joined.before?.epoch === left.epoch && joined.after?.instance_id === right?.instance_id
    && joined.after?.epoch === right?.epoch && right?.agent_id === left.agent_id
    && Number.isSafeInteger(left.epoch) && left.epoch > 0 && Number.isSafeInteger(right.epoch)
    && right.epoch > left.epoch && right.instance_id !== left.instance_id
    && right.pod_uid !== left.pod_uid
    && ['a', 'b'].every(am => !boundary.predecessor_uids.includes(after.topology.ams[am].pod_uid))
    && after.topology.ams[right.am]?.instance_id === right.instance_id
    && after.topology.ams[right.am]?.pod_uid === right.pod_uid) {
    return {verified: true, mechanism: 'observed_parent_rollout', evidence_sha256: hash({before, after, boundary})};
  }
  const killed = boundary.killed_am;
  if (boundary.owner_container_exit_observed !== true || !['a', 'b'].includes(killed)
    || left?.am !== killed || right?.am === killed || !['a', 'b'].includes(right?.am)
    || left.agent_id !== right.agent_id || !left.agent_id
    || !Number.isSafeInteger(left.epoch) || left.epoch <= 0
    || !Number.isSafeInteger(right.epoch) || right.epoch <= left.epoch
    || left.pod_uid === right.pod_uid || left.instance_id === right.instance_id
    || before.topology.ams[killed].pod_uid === after.topology.ams[killed].pod_uid
    || after.topology.ams[right.am].pod_uid !== right.pod_uid
    || before.topology.ams[left.am].pod_uid !== left.pod_uid) return null;
  return {verified: true, evidence_sha256: hash({before, after, boundary})};
}

export function parentTransport(config, services) {
  const roles = new Map([['user', config.account_lease?.actor ?? 'user02']]);
  if (config.admin_bearer) roles.set('admin', 'admin');
  for (const [name, actor] of Object.entries(config.actors ?? {})) {
    if (actor?.bearer) roles.set(name, name);
  }
  if (!config.bearer) roles.delete('user');
  return {hasActor: actor => roles.has(actor),
    async request({actor, am, method, path, ...options}) {
      if (!roles.has(actor)) throw new Blocked('actor_unavailable_' + actor.replaceAll('-', '_'));
      check(typeof services?.request === 'function', 'parent_request_service_required');
      return services.request(roles.get(actor), am, method, path, options);
    }};
}

export async function createParticipant(config, {registry: suppliedRegistry} = {}) {
  check(config.scope === 'LOCAL_DISPOSABLE_TWO_AM' && config.database_disposable === true
    && config.single_matrix_writer === true && config.restart_authority === 'PARENT_ONLY',
  'coverage_parent_lease_required');
  const registry = suppliedRegistry ?? await loadRegistry();
  const source = config.expected_api_source, image = config.expected_api_image;
  check(/^[0-9a-f]{40}$/.test(source ?? '') && /@sha256:[0-9a-f]{64}$/.test(image ?? ''),
    'coverage_parent_image_required');
  const requested = config.executable_case_ids ?? [...registry.cases.keys()];
  check(Array.isArray(requested) && new Set(requested).size === requested.length
    && requested.every(id => registry.rows.has(id)), 'coverage_selection_invalid');
  const results = [], states = new Map(), phases = new Set();
  let active = false, closed = false, before, lastServices;
  const binding = {source, image, campaign_id: config.run_id, fixtures: []};
  return {
    async runPhase(context) {
      const {phase, adapter, services} = context;
      check(!closed && !active && ['before_restart', 'after_restart'].includes(phase)
        && !phases.has(phase) && (phase === 'before_restart' || phases.has('before_restart')),
      'coverage_phase_order');
      active = true; phases.add(phase); lastServices = services;
      try {
        const topology = validateTopology(await adapter.topology(), image);
        const witness = context.record?.agentId;
        check(typeof witness === 'string' && witness.length > 0, 'coverage_restart_witness_required');
        const observation = {topology, owner: await adapter.ownerOf({agentId: witness})};
        check(observation.owner?.agent_id === witness, 'coverage_restart_witness_identity');
        await services.record({kind: 'executable_topology', phase, observation});
        if (phase === 'before_restart') before = observation;
        else binding.owner_restart = verifyRestart(before, observation, context.restart);
        const transport = parentTransport(config, services);
        const shared = await bindParentFixtures(config, context, transport);
        // This participant can safely seed uniquely named files in the parent's
        // own witness only when it holds that exact account lease.
        if (config.account_lease?.actor === 'user02' && config.runtime_images?.base) {
          shared.primitives.fileAgent ??= {agentId: witness, actor: 'user', ownedByRun: true,
            runtimeImage: config.runtime_images.base};
          shared.primitives.runtimeFor ??= args => adapter.runtimeFor(args);
        }
        const installed = new Set(Object.keys(shared.fixtures ?? {}));
        binding.executable_case_ids = services.executable?.readyCaseIds ?? [];
        for (const id of requested) {
          const definition = registry.cases.get(id), row = registry.rows.get(id);
          if (!definition) continue; // Counted as UNIMPLEMENTED, never a synthetic pass.
          const fixtures = fixturesForCase(registry, definition, shared);
          Object.keys(fixtures).forEach(name => installed.add(name));
          const minimum = row.phase_minimum_runs?.[phase] ?? (phase === 'before_restart' ? 3 : 1);
          const runState = states.get(id) ?? {}; states.set(id, runState);
          for (let iteration = 1; iteration <= minimum; iteration++) {
            const result = await executeCase(definition, row, {phase, iteration,
              am: iteration % 2 ? 'a' : 'b', source, image, fixtures, adapter, transport,
              campaignId: config.run_id, runState, record: event => services.record(event)});
            results.push(result);
            // Missing setup or a failed intent stays in the denominator. Do not
            // repeat a possibly accepted mutation just to obtain a green retry.
            if (result.status !== 'PASS') break;
          }
        }
        binding.fixtures = [...installed];
        const counts = coverageCounts(registry, results, binding);
        await services.record({kind: 'executable_counts', counts});
        return {verdict: results.some(row => row.status === 'FAIL') ? 'FAIL' : 'PARTIAL',
          counts, completed_phases: [...phases], runtime_acceptance: false};
      } finally { active = false; }
    },
    async finalize() {
      check(!active, 'coverage_phase_still_active'); closed = true;
      const counts = coverageCounts(registry, results, binding);
      if (lastServices) await lastServices.record({kind: 'executable_private_packet', binding, results});
      if (lastServices) await lastServices.record({kind: 'executable_final_counts', counts});
      return {verdict: results.some(row => row.status === 'FAIL') ? 'FAIL' : 'PASS',
        counts, runtime_acceptance: Object.values(counts.roles).every(role => role.passing === role.total),
        application_processes_started: 0, parent_cleanup_required: true};
    },
  };
}
