import { EventSourceParserStream } from 'eventsource-parser/stream';
import { CapacityClient } from './lc01-client.mjs';
import { check, hash, GateError } from './checks.mjs';
import { LocalCluster } from './kubernetes.mjs';
import { guardedFetch, validateConfig } from './safety.mjs';

const cursorPattern = /^ag1\.([a-f0-9]{64})\.(0|[1-9]\d*)$/;
const shaPattern = /^[a-f0-9]{64}$/;

function fixtureOwners(config, env) {
  const cohort = config.localCohort;
  check(cohort?.cluster && shaPattern.test(cohort.cluster.namespaceHash)
    && /^[a-f0-9]{40}$/.test(cohort.sourceSha) && hash(cohort.sourceSha) === config.sourceHash
    && Array.isArray(cohort.owners) && [2, 100].includes(cohort.owners.length)
    && hash(JSON.stringify(cohort.owners)) === config.fixtureHash,
  'local_cohort_binding_missing', true);

  const owners = cohort.owners.map((owner, index) => {
    check(/^agt_[A-Za-z0-9_-]{1,120}$/.test(owner?.agent)
      && /^sbx_[A-Za-z0-9_-]{1,120}$/.test(owner.sandbox) && shaPattern.test(owner.subjectHash),
    'local_cohort_owner_invalid', true);
    const bearer = env[`CMA_LOCAL_SCALE_BEARER_${index + 1}`];
    check(typeof bearer === 'string' && /^[\x21-\x7e]{16,4096}$/.test(bearer),
      'local_cohort_owner_credential_missing', true);
    return { agent: owner.agent, sandbox: owner.sandbox, subjectHash: owner.subjectHash, bearer };
  });
  for (const field of ['agent', 'sandbox', 'subjectHash', 'bearer']) {
    check(new Set(owners.map((owner) => owner[field])).size === owners.length,
      'local_cohort_owner_reused', true);
  }
  return owners;
}

async function readFixture(client, owner, signal) {
  const session = await client.json('/auth/session', signal);
  check(session?.authenticated === true && session.visitor === false
    && typeof session.subject === 'string' && hash(session.subject) === owner.subjectHash,
  'local_cohort_session_mismatch', true);
  const agent = await client.json(client.agentPath, signal);
  check(agent?.id === owner.agent && agent.status === 'ready' && agent.sandbox?.id === owner.sandbox,
    'local_cohort_agent_not_ready', true);
  const sandbox = await client.json(`/api/v1/sandboxes/${owner.sandbox}`, signal);
  check(sandbox?.id === owner.sandbox && sandbox.status === 'active'
    && typeof sandbox.owner === 'string' && hash(sandbox.owner) === owner.subjectHash,
  'local_cohort_sandbox_owner_mismatch', true);
  const head = await client.json(`${client.eventsPath}/head`, signal);
  const cursor = cursorPattern.exec(head?.through_cursor);
  check(head?.agent_id === owner.agent && head.active_turn === null && cursor
    && head.incarnation === cursor[1], 'local_cohort_history_not_idle', true);
  return head;
}

async function observeFixture(client, head, signal, connected, onEvent) {
  const query = new URLSearchParams({ after_cursor: head.through_cursor, follow: 'true' });
  const response = await client.request(`${client.eventsPath}?${query}`, { stream: true, signal });
  if (response.status !== 200 || !response.body
    || response.headers.get('content-type')?.split(';')[0].trim() !== 'text/event-stream') {
    await response.body?.cancel();
    check(false, 'local_cohort_stream_refused');
  }
  const events = response.body.pipeThrough(new TextDecoderStream('utf-8', { fatal: true }))
    .pipeThrough(new EventSourceParserStream({ onError: 'terminate', maxBufferSize: 1024 * 1024 }));
  let caughtUp = false;
  let sequence = BigInt(cursorPattern.exec(head.through_cursor)[2]);
  for await (const event of events) {
    const data = JSON.parse(event.data);
    check(data?.agent_id === head.agent_id && data.incarnation === head.incarnation,
      'local_cohort_stream_identity_changed');
    if (!caughtUp) {
      check(event.event === 'cma.agent.live_gap' && event.id === undefined
        && data.reason === 'reconnected' && data.frames === null && data.turn_sequence === null,
      'local_cohort_stream_not_idle');
      caughtUp = true;
      connected();
    } else if (event.event === 'cma.agent.event') {
      const next = cursorPattern.exec(event.id);
      check(next && next[1] === head.incarnation && event.id === data.cursor
        && Number.isSafeInteger(data.sequence) && BigInt(data.sequence) === sequence + 1n
        && BigInt(next[2]) === BigInt(data.sequence), 'local_cohort_cursor_gap');
      sequence = BigInt(data.sequence);
      onEvent(data);
    } else {
      check(event.event === 'cma.agent.live' && event.id === undefined
        && Number.isSafeInteger(data.turn_sequence) && Number.isSafeInteger(data.after_sequence)
        && data.turn_sequence > 0 && data.after_sequence >= data.turn_sequence
        && BigInt(data.after_sequence) <= sequence, 'local_cohort_stream_gap');
    }
  }
  check(signal.aborted, 'local_cohort_stream_closed');
}

// Keep the same authenticated Agent streams open from readiness through the
// entire callback and fail the run if any disconnects or loses a cursor.
export async function withLocalCohort(config, use, { env = process.env, fetchImpl, command, onEvent, signal: externalSignal } = {}) {
  validateConfig(config);
  check(typeof config.endpoints.am === 'string', 'local_cohort_am_missing', true);
  externalSignal?.throwIfAborted();
  const owners = fixtureOwners(config, env);
  const { cluster, sourceSha } = config.localCohort;
  await new LocalCluster({ ...cluster, context: config.context, namespace: config.namespace }, command).verify();
  const fetcher = guardedFetch(fetchImpl);
  const clients = owners.map((owner) => new CapacityClient(config.endpoints.am, owner, { fetcher }));
  const controller = new AbortController();
  const signal = externalSignal ? AbortSignal.any([externalSignal, controller.signal]) : controller.signal;
  const observers = [];
  const verifyBuild = async () => {
    const build = await clients[0].json('/api/v1/build-info', signal);
    check(build?.source_sha === sourceSha, 'local_cohort_build_changed', true);
  };
  const timer = setTimeout(() => controller.abort(new GateError('local_cohort_deadline_exceeded')), config.scaleTimeoutMs);
  try {
    await verifyBuild();
    for (let offset = 0; offset < clients.length; offset += 10) {
      signal.throwIfAborted();
      const batch = clients.slice(offset, offset + 10);
      const pending = batch.map((client, index) => {
        const completion = Promise.withResolvers();
        const ordinal = offset + index;
        const observer = readFixture(client, owners[ordinal], signal)
          .then((head) => observeFixture(client, head, signal, completion.resolve,
            (event) => onEvent?.(ordinal, event)))
          .catch((error) => {
            controller.abort(error);
            completion.reject(error);
          });
        observers.push(observer);
        return completion.promise;
      });
      await Promise.all(pending);
    }
    clearTimeout(timer);
    signal.throwIfAborted();
    const result = await use({ clients, signal, sourceSha, fixtureHash: config.fixtureHash });
    signal.throwIfAborted();
    // The final probe has its own bound because the readiness timer has ended.
    const build = await clients[0].json('/api/v1/build-info', AbortSignal.any([signal, AbortSignal.timeout(15_000)]));
    check(build?.source_sha === sourceSha, 'local_cohort_build_changed', true);
    signal.throwIfAborted();
    return result;
  } catch (error) {
    throw signal.aborted ? signal.reason : error;
  } finally {
    clearTimeout(timer);
    controller.abort();
    await Promise.all(observers);
  }
}

export function verifyLocalCohort(config, dependencies) {
  return withLocalCohort(config, ({ clients, sourceSha, fixtureHash }) => ({
    verifiedOwners: clients.length,
    simultaneousStreams: clients.length,
    sourceSha,
    fixtureHash,
  }), dependencies);
}
