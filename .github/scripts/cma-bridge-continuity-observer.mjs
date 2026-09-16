// Read-only evidence for #850. This observer does not authorize deployment or
// replace the disposable live-turn/browser/fault campaign required by #873.
import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
import { pathToFileURL } from 'node:url';
import { check, errorCode } from './cma-bridge-continuity-errors.mjs';
import { transport } from './cma-bridge-continuity-transport.mjs';

const schema = 'cma-bridge-continuity-observation-v1';
const id = /^(agt|sbx|wks|wkt)_[A-Za-z0-9_-]+$/;

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
  }
  return value;
}

export const digest = (value) => createHash('sha256').update(JSON.stringify(canonical(value))).digest('hex');

export function validateConfig(config) {
  check(config?.schema === 'cma-bridge-continuity-config-v1', 'invalid_config_schema');
  const origin = new URL(config.origin);
  check(origin.protocol === 'https:' && origin.origin === config.origin && !origin.username && !origin.password, 'invalid_origin');
  for (const key of ['context', 'namespace', 'deploymentId']) {
    check(typeof config[key] === 'string' && /^[A-Za-z0-9][A-Za-z0-9_.:/@-]{0,255}$/.test(config[key]), 'invalid_cluster_scope');
  }
  check(typeof config.tokenFile === 'string' && isAbsolute(config.tokenFile), 'absolute_token_file_required');
  check(Array.isArray(config.agentIds) && config.agentIds.length > 0 && config.agentIds.length <= 20, 'bounded_agent_scope_required');
  check(config.agentIds.every((value) => /^agt_[A-Za-z0-9_-]+$/.test(value)) && new Set(config.agentIds).size === config.agentIds.length, 'invalid_agent_scope');
  return config;
}

export function eventPrefix(events) {
  check(Array.isArray(events), 'invalid_events');
  return events.map((event, index) => {
    check(Number.isSafeInteger(event.seq) && event.seq === index + 1, 'event_sequence_gap_or_duplicate');
    check(typeof event.appended_at === 'string' && event.kind !== null && typeof event.kind === 'object', 'invalid_event');
    return { seq: event.seq, sha256: digest(event) };
  });
}

function validateSnapshot(value) {
  check(value?.schema === schema && value.status === 'observed', 'successful_observation_required');
  check(Number.isFinite(Date.parse(value.observedAt)), 'invalid_observation_time');
  check(Array.isArray(value.agents) && value.agents.length > 0, 'empty_observation');
  check(new Set(value.agents.map((agent) => agent.id)).size === value.agents.length, 'duplicate_agent_observation');
  check(Array.isArray(value.runtimes), 'runtime_observation_required');
  check(value.scope && Array.isArray(value.scope.agentIds) && digest([...value.scope.agentIds].sort()) === digest(value.agents.map((agent) => agent.id).sort()), 'incomplete_agent_scope');
  check(new Set(value.runtimes.map((runtime) => runtime.uid)).size === value.runtimes.length, 'duplicate_runtime_observation');
  for (const agent of value.agents) {
    check([agent.id, agent.sandboxId, agent.workspaceId, agent.worktreeId].every((value) => typeof value === 'string' && id.test(value)), 'invalid_lineage');
    check(Array.isArray(agent.events), 'events_required');
    check(/^[0-9a-f]{64}$/.test(agent.profileHash) && Number.isFinite(Date.parse(agent.createdAt)) && typeof agent.status === 'string', 'incomplete_agent_identity');
    for (const [index, event] of agent.events.entries()) {
      check(event.seq === index + 1 && /^[0-9a-f]{64}$/.test(event.sha256), 'invalid_event_prefix');
    }
  }
  for (const runtime of value.runtimes) {
    check(['name', 'uid', 'sandboxId', 'generation', 'contract'].every((key) => typeof runtime[key] === 'string' && runtime[key].length > 0), 'incomplete_runtime_identity');
    check(Array.isArray(runtime.containers) && runtime.containers.length > 0, 'runtime_containers_required');
    check(runtime.containers.every((container) => ['name', 'image', 'id'].every((key) => typeof container[key] === 'string' && container[key].length > 0) && Number.isSafeInteger(container.restarts) && container.restarts >= 0), 'incomplete_container_identity');
  }
  for (const agent of value.agents.filter((agent) => ['ready', 'working', 'waiting_approval'].includes(agent.status))) {
    check(value.runtimes.filter((runtime) => runtime.sandboxId === agent.sandboxId).length === 1, 'live_agent_runtime_missing_or_ambiguous');
  }
}

export function compareSnapshots(before, after) {
  validateSnapshot(before);
  validateSnapshot(after);
  check(digest(before.scope) === digest(after.scope), 'observation_scope_changed');
  check(Date.parse(after.observedAt) >= Date.parse(before.observedAt), 'observation_time_regressed');
  const failures = [];
  const retained = new Map(after.agents.map((agent) => [agent.id, agent]));
  for (const previous of before.agents) {
    const current = retained.get(previous.id);
    if (!current) { failures.push({ code: 'agent_missing', id: previous.id }); continue; }
    for (const key of ['sandboxId', 'workspaceId', 'worktreeId', 'profileHash', 'createdAt']) {
      if (current[key] !== previous[key]) failures.push({ code: 'agent_identity_changed', id: previous.id, field: key });
    }
    if (current.events.length < previous.events.length || previous.events.some((event, index) => digest(event) !== digest(current.events[index]))) {
      failures.push({ code: 'history_prefix_changed', id: previous.id });
    }
    if (['ready', 'working', 'waiting_approval'].includes(previous.status) && !['ready', 'working', 'waiting_approval'].includes(current.status)) {
      failures.push({ code: 'live_agent_became_unavailable', id: previous.id });
    }
  }
  const runtimes = new Map(after.runtimes.map((runtime) => [runtime.uid, runtime]));
  for (const previous of before.runtimes) {
    const current = runtimes.get(previous.uid);
    if (!current) failures.push({ code: 'runtime_replaced_or_missing', id: previous.sandboxId });
    else if (digest(current) !== digest(previous)) failures.push({ code: 'runtime_changed', id: previous.sandboxId });
  }
  return {
    schema: 'cma-bridge-continuity-comparison-v1',
    status: failures.length ? 'failed' : 'passed',
    before: before.observedAt,
    after: after.observedAt,
    coverage: ['public_agent_lineage', 'profile_pins', 'durable_event_prefix', 'runtime_pod_and_container_identity'],
    unproved: ['continuous_availability_between_samples', 'native_conversation_identity', 'dirty_files', 'duplicate_runtime_input', 'old_browser_drafts', 'automatic_stream_reconnect', 'cleanup_safety'],
    failures,
  };
}

export async function capture(config, { get, pods }, now = () => new Date()) {
  validateConfig(config);
  // Exercise the real public web and authenticated API, without changing an
  // Agent's activity, issuing runtime commands, or reading private file bodies.
  await get('/', false);
  const agents = [];
  for (const agentId of config.agentIds) {
    const agent = await get(`/api/v1/agents/${agentId}`, true);
    check(agent.id === agentId, 'agent_identity_mismatch');
    const events = [];
    let after = 0;
    for (let page = 0; ; page += 1) {
      check(page < 100, 'event_observation_limit');
      const result = await get(`/api/v1/agents/${agentId}/events?after=${after}&limit=100`, true);
      check(Array.isArray(result.items) && result.items.length <= 100, 'invalid_event_page');
      events.push(...result.items);
      if (result.next === undefined || result.next === null) break;
      check(Number.isSafeInteger(result.next) && result.next > after && result.next === events.at(-1)?.seq, 'event_cursor_did_not_advance');
      after = result.next;
    }
    agents.push({
      id: agent.id, sandboxId: agent.sandbox.id, workspaceId: agent.workspace.id,
      worktreeId: agent.worktree.id, profileHash: digest(agent.profile),
      createdAt: agent.created_at, status: agent.status, events: eventPrefix(events),
    });
  }
  const sandboxIds = new Set(agents.map((agent) => agent.sandboxId));
  const inventory = await pods();
  check(Array.isArray(inventory.items), 'invalid_runtime_inventory');
  const runtimes = inventory.items.filter((pod) => sandboxIds.has(pod.metadata.labels?.['cma/sandbox'])).map((pod) => {
    check(pod.status.phase === 'Running' && !pod.metadata.deletionTimestamp, 'runtime_not_running');
    check(pod.status.containerStatuses?.length === pod.spec.containers.length && pod.status.containerStatuses.every((container) => container.ready), 'runtime_not_ready');
    return {
      name: pod.metadata.name, uid: pod.metadata.uid, sandboxId: pod.metadata.labels['cma/sandbox'],
      generation: pod.metadata.labels['cma/runtime-generation'], contract: pod.metadata.labels['cma/runtime-contract'],
      containers: pod.status.containerStatuses.map((container) => ({ name: container.name, image: container.imageID, id: container.containerID, restarts: container.restartCount })).sort((a, b) => a.name.localeCompare(b.name)),
    };
  });
  for (const agent of agents.filter((agent) => ['ready', 'working', 'waiting_approval'].includes(agent.status))) {
    check(runtimes.filter((runtime) => runtime.sandboxId === agent.sandboxId).length === 1, 'live_agent_runtime_missing_or_ambiguous');
  }
  const result = {
    schema, status: 'observed', observedAt: now().toISOString(),
    scope: { origin: config.origin, context: config.context, namespace: config.namespace, deploymentId: config.deploymentId, agentIds: [...config.agentIds].sort() },
    agents, runtimes,
  };
  validateSnapshot(result);
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [action, first, second] = process.argv.slice(2);
    check(process.argv.length === 5 && ['capture', 'compare'].includes(action), 'usage: capture CONFIG OUTPUT | compare BEFORE AFTER');
    if (action === 'capture') {
      const config = validateConfig(JSON.parse(await readFile(first, 'utf8')));
      const result = await capture(config, await transport(config));
      // Never overwrite an earlier receipt, including one from a failed run.
      await writeFile(second, `${JSON.stringify(result, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
      console.log(JSON.stringify({ status: 'observed', agents: result.agents.length, runtimes: result.runtimes.length }));
    } else {
      const result = compareSnapshots(JSON.parse(await readFile(first, 'utf8')), JSON.parse(await readFile(second, 'utf8')));
      console.log(JSON.stringify(result, null, 2));
      if (result.status !== 'passed') process.exitCode = 1;
    }
  } catch (error) {
    // Network/provider exception text can carry private request details.
    console.error(JSON.stringify({ status: 'failed', code: errorCode(error) }));
    process.exitCode = 1;
  }
}
