import { randomUUID } from 'node:crypto';
import { performance } from 'node:perf_hooks';
import { setTimeout as sleep } from 'node:timers/promises';
import { check, safeFailure } from './checks.mjs';
import { CapacityClient } from './lc01-client.mjs';
import { end, loadPlan, phase, steadyEnd, steadyStart, turnDeadlineMs, workloadShape } from './lc01-plan.mjs';

export function* deliveryPlan(config) {
  const background = config.background.map((row, index) => ({
    at: steadyStart, interval: 1000 / (1.5 * row.peakReadsPerSecond), index,
  }));
  for (const entry of loadPlan()) {
    for (;;) {
      const next = background.reduce((earlier, later) => earlier.at <= later.at ? earlier : later);
      if (next.at > entry.at || next.at >= steadyEnd) break;
      yield { at: next.at, kind: 'background', user: next.index };
      next.at += next.interval;
    }
    yield entry;
  }
}

export async function runCapacity(config, write, signal) {
  const run = randomUUID();
  const now = () => performance.now();
  const clients = config.users.map((user) => new CapacityClient(config.origin, user, { now }));
  const background = config.background.map((row) => new CapacityClient(row.origin, row, { now }));
  // Only publish the declared proof fields. The reservation file remains private.
  const { cohortHash, backgroundPeakHash, placementHash, runtimeMappingHash, providerBudgetHash,
    verifiedK, nPlusOneVerified } = config.reservation;
  const { sourceSha, imageDigest } = config.candidate;
  write({ type: 'run', run, candidate: { sourceSha, imageDigest }, fixtureSha256: config.fixtureSha256,
    reservation: { cohortHash, backgroundPeakHash, placementHash, runtimeMappingHash, providerBudgetHash,
      verifiedK, nPlusOneVerified, manifestHash: workloadShape(config) },
    users: config.users.map(({ subjectHash, providerClass, modelClass, runtimeEvidenceHash, delaySeconds }) =>
      ({ subjectHash, providerClass, modelClass, runtimeEvidenceHash, delaySeconds })),
    background: config.background.map(({ consumer, peakReadsPerSecond }) => ({ consumer, peakReadsPerSecond })),
  });
  const verifyBuild = async () => {
    const versions = await Promise.all(clients.slice(0, 5).map((client) => client.json('/api/v1/build-info')));
    check(versions.every((build) => build.source_sha === config.candidate.sourceSha), 'staging_build_changed', true);
  };
  await verifyBuild();
  const cursors = [];
  for (let offset = 0; offset < clients.length; offset += 5) {
    signal.throwIfAborted();
    cursors.push(...await Promise.all(clients.slice(offset, offset + 5)
      .map((client) => client.preflight(config.fixtureSha256))));
  }
  await Promise.all(background.map(async (client) => {
    check((await client.read()).id === client.fixture.agent, 'background_identity_changed', true);
  }));
  const started = now();
  write({ type: 'start', utc: new Date().toISOString() });
  const observers = new Map();
  const unresolved = new Set();
  const pending = new Set();
  const record = (type, data) => write({ type, ms: now() - started, ...data });

  const track = (task) => {
    pending.add(task);
    void task.then(() => pending.delete(task), () => pending.delete(task));
    return task;
  };

  function connect({ user }) {
    const controller = new AbortController();
    observers.set(user, controller);
    track(clients[user].observe(cursors[user], AbortSignal.any([controller.signal, signal]),
      (event, data) => record(event, { user, ...data })));
  }

  function turn({ user, at, protocol }) {
    const observe = (event, data = {}) => record(event, { user, at, protocol, phase: phase(at), ...data });
    const age = now() - started - at;
    observe('offered', { queueMs: age });
    if (unresolved.has(user) || age >= turnDeadlineMs) {
      observe('overdue', { reason: unresolved.has(user) ? 'prior_turn_unresolved' : 'generator_late' });
      return;
    }
    unresolved.add(user);
    track(clients[user].turn({ run, user, at, protocol, record: observe, fixtureSha: config.fixtureSha256,
      started: started + at, signal }).then((settled) => {
      if (settled) unresolved.delete(user);
    }, (error) => observe('turn_unresolved', { reason: safeFailure(error).code })));
  }

  let pendingReads = 0;
  function read({ kind, user, at }) {
    const isBackground = kind === 'background';
    const client = isBackground ? background[user] : clients[user];
    const consumer = isBackground ? client.fixture.consumer : 'public';
    const complete = (http, failure) => record('read', { kind, user, at, consumer, http, failure,
      latencyMs: now() - started - at });
    if (pendingReads >= 200) {
      complete(null, 'generator_overloaded');
      return;
    }
    pendingReads += 1;
    track(client.read().then((value) => {
      check(value.id === client.fixture.agent, 'read_identity_changed');
      complete(200, null);
    }).catch((error) => {
      const code = safeFailure(error).code;
      complete(/^http_[1-5][0-9]{2}$/.test(code) ? Number(code.slice(5)) : null, code);
    }).finally(() => {
      pendingReads -= 1;
    }));
  }

  try {
    for (const entry of deliveryPlan(config)) {
      signal.throwIfAborted();
      while (started + entry.at > now()) {
        await sleep(Math.ceil(started + entry.at - now()), undefined, { signal });
      }
      switch (entry.kind) {
        case 'connect': connect(entry); break;
        case 'disconnect': observers.get(entry.user).abort(); break;
        case 'turn': turn(entry); break;
        default: read(entry);
      }
    }
  } finally {
    for (const observer of observers.values()) observer.abort();
    await Promise.allSettled(pending);
  }
  await verifyBuild();
  check(now() - started >= end, 'load_window_ended_early');
  write({ type: 'end', utc: new Date().toISOString(), ms: now() - started,
    unresolvedOwners: unresolved.size });
}
