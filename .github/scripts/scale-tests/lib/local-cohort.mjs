import { EventSourceParserStream } from 'eventsource-parser/stream';
import { CapacityClient } from './lc01-client.mjs';
import { check, hash, withinDeadline } from './checks.mjs';
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

async function observeFixture(client, head, signal, connected) {
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
  for await (const event of events) {
    const data = JSON.parse(event.data);
    check(data?.agent_id === head.agent_id && data.incarnation === head.incarnation,
      'local_cohort_stream_identity_changed');
    check(!caughtUp && event.event === 'cma.agent.live_gap'
      && data.reason === 'reconnected' && data.frames === null && data.turn_sequence === null,
    'local_cohort_stream_not_idle');
    caughtUp = true;
    connected();
  }
  check(signal.aborted, 'local_cohort_stream_closed');
}

// Establish the initial cohort. The streaming and load scenarios build on this
// boundary and must account separately for continuity after the first catch-up.
export async function verifyLocalCohort(config, { env = process.env, fetchImpl, command } = {}) {
  validateConfig(config);
  check(typeof config.endpoints.am === 'string', 'local_cohort_am_missing', true);
  const owners = fixtureOwners(config, env);
  const { cluster, sourceSha } = config.localCohort;
  await new LocalCluster({ ...cluster, context: config.context, namespace: config.namespace }, command).verify();
  const fetcher = guardedFetch(fetchImpl);
  const clients = owners.map((owner) => new CapacityClient(config.endpoints.am, owner, { fetcher }));

  return withinDeadline(async (deadline) => {
    const controller = new AbortController();
    const signal = AbortSignal.any([deadline, controller.signal]);
    const observers = [];
    const verifyBuild = async () => {
      const build = await clients[0].json('/api/v1/build-info', signal);
      check(build?.source_sha === sourceSha, 'local_cohort_build_changed', true);
    };
    try {
      await verifyBuild();
      for (let offset = 0; offset < clients.length; offset += 10) {
        const batch = clients.slice(offset, offset + 10);
        const pending = batch.map((client, index) => {
          const completion = Promise.withResolvers();
          observers.push(readFixture(client, owners[offset + index], signal)
            .then((head) => observeFixture(client, head, signal, completion.resolve)));
          return completion.promise;
        });
        await Promise.race([Promise.all(pending), ...observers]);
      }
      await Promise.race([verifyBuild(), ...observers]);
      signal.throwIfAborted();
      return {
        verifiedOwners: owners.length,
        simultaneousStreams: observers.length,
        sourceSha,
        fixtureHash: config.fixtureHash,
      };
    } finally {
      controller.abort();
      await Promise.allSettled(observers);
    }
  }, config.scaleTimeoutMs, 'local_cohort_deadline_exceeded');
}
