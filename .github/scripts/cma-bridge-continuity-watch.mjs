// Bounded sampled evidence for #850. Findings describe what was observed; they
// do not decide whether a temporary API interruption or deployment is allowed.
import { open, readFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { capture, compareSnapshots, digest, validateConfig } from './cma-bridge-continuity-observer.mjs';
import { check, errorCode, ObservationError, requireActive } from './cma-bridge-continuity-errors.mjs';
import { transport } from './cma-bridge-continuity-transport.mjs';

const schema = 'cma-bridge-continuity-watch-v1';
const clock = {
  now: () => new Date(), monotonic: () => performance.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms), clearTimeout: (timer) => clearTimeout(timer),
};

export function validateWatchConfig(config) {
  validateConfig(config);
  const value = config.watch;
  check(value && ['durationMs', 'intervalMs', 'sampleTimeoutMs'].every((key) => Number.isSafeInteger(value[key])), 'bounded_watch_required');
  check(value.durationMs >= 1000 && value.durationMs <= 3_600_000, 'invalid_watch_duration');
  check(value.intervalMs >= 250 && value.intervalMs <= 60_000 && value.intervalMs <= value.durationMs, 'invalid_watch_interval');
  check(value.sampleTimeoutMs >= 250 && value.sampleTimeoutMs <= 60_000 && value.sampleTimeoutMs <= value.durationMs, 'invalid_sample_timeout');
  check(Math.ceil(value.durationMs / value.intervalMs) <= 1200, 'watch_sample_limit');
  return config;
}

function pause(ms, signal, timer) {
  requireActive(signal);
  return new Promise((resolve, reject) => {
    const done = () => { signal.removeEventListener('abort', abort); resolve(); };
    const handle = timer.setTimeout(done, ms);
    const abort = () => {
      timer.clearTimeout(handle);
      signal.removeEventListener('abort', abort);
      try { requireActive(signal); } catch (error) { reject(error); }
    };
    signal.addEventListener('abort', abort, { once: true });
    if (signal.aborted) abort();
  });
}

function retainedSnapshot(snapshot) {
  return {
    ...snapshot,
    agents: snapshot.agents.map((agent) => ({
      ...agent, events: { cursor: agent.events.at(-1)?.seq ?? 0, prefixSha256: digest(agent.events) },
    })),
  };
}

function publicProbe(path) {
  if (path === '/') return { kind: 'public_web' };
  const [, , , , agentId, endpoint] = path.split('/');
  return endpoint ? { kind: 'public_events', agentId, after: Number(new URL(path, 'https://scope.invalid').searchParams.get('after')) }
    : { kind: 'public_agent', agentId };
}

export async function watch(config, reads, { signal, timer = clock } = {}) {
  validateWatchConfig(config);
  const { durationMs, intervalMs, sampleTimeoutMs } = config.watch;
  const started = timer.monotonic();
  const offset = () => Math.max(0, timer.monotonic() - started);
  const run = new AbortController();
  const cancel = () => run.abort(new ObservationError('observation_cancelled'));
  signal?.addEventListener('abort', cancel, { once: true });
  if (signal?.aborted) cancel();
  const deadline = () => run.abort(new ObservationError('watch_deadline'));
  const handle = timer.setTimeout(deadline, durationMs);
  const active = () => {
    if (offset() >= durationMs && !run.signal.aborted) deadline();
    requireActive(run.signal);
  };
  const result = {
    schema, status: 'running', startedAt: timer.now().toISOString(),
    scope: { origin: config.origin, context: config.context, namespace: config.namespace, deploymentId: config.deploymentId, agentIds: [...config.agentIds].sort() },
    schedule: { durationMs, intervalMs, sampleTimeoutMs, overlappingSamples: false },
    samples: [], gaps: [], findings: [],
    coverage: ['sampled_public_read_availability_and_latency', 'durable_event_prefix_and_cursor', 'runtime_pod_and_container_identity'],
    unproved: ['continuous_availability_between_samples', 'native_conversation_identity', 'dirty_files', 'duplicate_runtime_input', 'old_browser_drafts', 'automatic_stream_reconnect', 'cleanup_safety', 'deployment_or_rollback_acceptance'],
  };
  let next = 0;
  let previous;
  let lastFinished = 0;
  try {
    while (next < durationMs) {
      active();
      if (next > offset()) await pause(next - offset(), run.signal, timer);
      active();
      const sample = { index: result.samples.length, scheduledOffsetMs: next, startedOffsetMs: offset(), startedAt: timer.now().toISOString(), probes: [] };
      if (sample.startedOffsetMs > lastFinished) result.gaps.push({ fromOffsetMs: lastFinished, toOffsetMs: sample.startedOffsetMs, code: 'between_samples' });
      const local = new AbortController();
      const sampleSignal = AbortSignal.any([run.signal, local.signal]);
      const sampleTimer = timer.setTimeout(() => local.abort(new ObservationError('sample_timeout')), sampleTimeoutMs);
      const probe = async (identity, read) => {
        active();
        requireActive(sampleSignal);
        const measurement = { ...identity, startedOffsetMs: offset(), startedAt: timer.now().toISOString() };
        try {
          const value = await read(sampleSignal);
          active();
          requireActive(sampleSignal);
          measurement.status = 'observed';
          return value;
        } catch (error) {
          measurement.status = 'error';
          measurement.code = errorCode(sampleSignal.aborted ? sampleSignal.reason : error);
          throw error;
        } finally {
          measurement.durationMs = offset() - measurement.startedOffsetMs;
          sample.probes.push(measurement);
        }
      };
      try {
        const snapshot = await capture(config, {
          get: (path, authenticated) => probe(publicProbe(path), (sampleSignal) => reads.get(path, authenticated, { signal: sampleSignal })),
          pods: () => probe({ kind: 'runtime_inventory' }, (sampleSignal) => reads.pods({ signal: sampleSignal })),
        }, timer.now);
        active();
        requireActive(sampleSignal);
        sample.status = 'observed';
        sample.snapshot = retainedSnapshot(snapshot);
        if (previous) {
          sample.comparison = compareSnapshots(previous, snapshot);
          for (const finding of sample.comparison.failures) result.findings.push({ sample: sample.index, ...finding });
        }
        previous = snapshot;
      } catch (error) {
        sample.status = 'error';
        sample.code = errorCode(sampleSignal.aborted ? sampleSignal.reason : error);
        result.findings.push({ sample: sample.index, code: sample.code });
      } finally {
        timer.clearTimeout(sampleTimer);
        sample.finishedOffsetMs = offset();
        sample.finishedAt = timer.now().toISOString();
        sample.durationMs = sample.finishedOffsetMs - sample.startedOffsetMs;
        lastFinished = sample.finishedOffsetMs;
        if (sample.status === 'error') result.gaps.push({ fromOffsetMs: sample.startedOffsetMs, toOffsetMs: sample.finishedOffsetMs, sample: sample.index, code: 'incomplete_sample' });
        result.samples.push(sample);
      }
      active();
      next += intervalMs;
      if (next < offset()) {
        const from = next;
        const slots = Math.ceil((offset() - next) / intervalMs);
        next += slots * intervalMs;
        result.gaps.push({ fromOffsetMs: from, toOffsetMs: Math.min(next, durationMs), skippedSlots: slots, code: 'sample_overran_interval' });
      }
    }
    if (offset() < durationMs) await pause(durationMs - offset(), run.signal, timer);
  } catch (error) {
    if (!run.signal.aborted) throw error;
  } finally {
    timer.clearTimeout(handle);
    signal?.removeEventListener('abort', cancel);
  }
  result.status = errorCode(run.signal.reason) === 'observation_cancelled' ? 'cancelled' : 'completed';
  result.finishedAt = timer.now().toISOString();
  result.elapsedMs = offset();
  if (lastFinished < result.elapsedMs) result.gaps.push({ fromOffsetMs: lastFinished, toOffsetMs: result.elapsedMs, code: 'between_samples' });
  result.observedSamples = result.samples.filter((sample) => sample.status === 'observed').length;
  result.errorSamples = result.samples.length - result.observedSamples;
  result.assessment = result.findings.length ? 'findings_recorded' : (result.observedSamples ? 'no_findings_at_sampled_times' : 'no_complete_observation');
  return result;
}

export async function saveWatch(config, output, { makeTransport = transport, ...options } = {}) {
  validateWatchConfig(config);
  // Reserve before reads; an existing receipt (including a symlink) fails closed.
  const file = await open(output, 'wx', 0o600);
  try {
    let result;
    try { result = await watch(config, await makeTransport(config), options); }
    catch (error) { result = { schema, status: 'failed', code: errorCode(error) }; }
    await file.writeFile(`${JSON.stringify(result, null, 2)}\n`);
    await file.sync();
    return result;
  } finally { await file.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const stop = new AbortController();
  const cancel = () => stop.abort();
  process.once('SIGINT', cancel);
  process.once('SIGTERM', cancel);
  try {
    check(process.argv.length === 4, 'watch_config_and_output_required');
    const config = JSON.parse(await readFile(process.argv[2], 'utf8'));
    const result = await saveWatch(config, process.argv[3], { signal: stop.signal });
    console.log(JSON.stringify({ status: result.status, assessment: result.assessment, observedSamples: result.observedSamples, errorSamples: result.errorSamples, findings: result.findings?.length, code: result.code }));
    if (result.status !== 'completed') process.exitCode = 1;
  } catch (error) {
    console.error(JSON.stringify({ status: 'failed', code: errorCode(error) }));
    process.exitCode = 1;
  } finally {
    process.removeListener('SIGINT', cancel);
    process.removeListener('SIGTERM', cancel);
  }
}
