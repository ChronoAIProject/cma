import { randomUUID } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { check, clock, hash, safeFailure } from './checks.mjs';
import { consumeEvents } from './lc01-client.mjs';
import { connectedWorkload, startLoad } from './load.mjs';
import { withLocalCohort } from './local-cohort.mjs';

const minute = 60_000;
const idPattern = /^[A-Za-z0-9_-]{1,128}$/;
const turnTimeoutMs = 120_000;
const readTimeoutMs = 15_000;
const turnInput = 'Reply with the single word OK. Do not call tools.';

async function submitTurn(client, attempt, key, signal, now, observeCompletion) {
  const response = await client.request(`/api/v2/agents/${client.fixture.agent}/responses`, {
    method: 'POST',
    body: { input: turnInput, stream: true },
    key, stream: true, signal,
  });
  attempt.http = response.status;
  const commandId = response.headers.get('x-cma-command-id');
  const responseId = response.headers.get('x-cma-response-id');
  // Retain even a failed admission's receipt when the service returned one.
  if (idPattern.test(commandId ?? '')) attempt.commandHash = hash(commandId);
  if (idPattern.test(responseId ?? '')) attempt.responseHash = hash(responseId);
  if (response.status !== 200 || !attempt.commandHash || !attempt.responseHash
    || response.headers.get('content-type')?.split(';')[0].trim() !== 'text/event-stream') {
    await response.body?.cancel();
    check(false, 'local_workload_turn_admission_failed');
  }
  attempt.admittedMs = now();

  let lastSequence = -1;
  let completed = false;
  await consumeEvents(response.body, (event) => {
    const { sequence_number: sequence, type } = event;
    check(Number.isSafeInteger(sequence) && sequence > lastSequence, 'local_workload_response_stream_sequence');
    lastSequence = sequence;
    if (event.response?.id !== undefined || event.response_id !== undefined) {
      check((event.response?.id ?? event.response_id) === responseId, 'local_workload_response_stream_identity');
    }
    check(!['error', 'response.failed', 'response.incomplete'].includes(type), 'local_workload_response_failed');
    if (type === 'response.completed') {
      check(event.response?.id === responseId && event.response.status === 'completed', 'local_workload_terminal_missing');
      completed = true;
      return false;
    }
  });
  check(completed, 'local_workload_terminal_missing');

  const [command, durable] = await Promise.all([
    client.json(`/api/v2/commands/${commandId}`, signal),
    client.json(`/api/v2/responses/${responseId}`, signal),
  ]);
  check(command?.receipt?.command_id === commandId && command.receipt.agent_id === client.fixture.agent
    && command.receipt.response_id === responseId && command.status === 'completed'
    && Number.isSafeInteger(command.turn_sequence) && command.turn_sequence > 0, 'local_workload_command_mismatch');
  check(durable?.id === responseId && durable.conversation?.id === client.fixture.agent
    && durable.status === 'completed', 'local_workload_durable_response_mismatch');
  await observeCompletion(command.turn_sequence, signal);
}

function score(report) {
  const receipts = new Set();
  const buckets = Array.from({ length: report.durationMs / minute }, () => ({ reads: 0, turns: 0 }));
  for (const attempt of report.attempts) {
    const actual = attempt.kind === 'turn' ? attempt.admittedMs : attempt.startedMs;
    const bucket = buckets[Math.floor(actual / minute)];
    if (attempt.kind === 'turn') {
      if (attempt.commandHash) {
        check(!receipts.has(attempt.commandHash), 'local_workload_duplicate_command');
        receipts.add(attempt.commandHash);
      }
      if (bucket) bucket.turns++;
    } else if (bucket && attempt.status === 'PASS') {
      bucket.reads++;
    }
  }
  report.minuteRates = buckets.map(({ reads, turns }) => ({ readsPerSecond: reads / 60, turnsPerMinute: turns }));
  check(report.samples.every(({ ok }) => ok), 'local_workload_request_failed');
  check(report.minuteRates.every(({ readsPerSecond, turnsPerMinute }) =>
    readsPerSecond >= connectedWorkload.readsPerSecond && turnsPerMinute >= connectedWorkload.turnsPerMinute),
  'local_workload_rate_missed');
}

export async function runLocalWorkload(config, { time = clock, ...dependencies } = {}) {
  const report = { schema: 'cma-local-workload-v1', runHash: hash(randomUUID()),
    startedAt: null, finishedAt: null, status: 'BLOCKED', attempts: [], samples: [] };
  const terminals = new Map();
  try {
    const durationMs = config?.localWorkload?.durationMs;
    check(Number.isSafeInteger(durationMs) && durationMs >= minute && durationMs <= 30 * minute
      && durationMs % minute === 0, 'local_workload_duration_invalid', true);
    check(config.localCohort?.owners?.length === connectedWorkload.users, 'local_workload_100_owners_required', true);
    report.durationMs = durationMs;
    report.planHash = hash(JSON.stringify({ ...connectedWorkload, durationMs, turnInput,
      read: 'agent_detail', turn: 'responses_v2' }));

    await withLocalCohort(config, async ({ clients, signal, sourceSha, fixtureHash }) => {
      Object.assign(report, { sourceSha, fixtureHash, workloadHash: config.workloadHash, openStreams: clients.length });
      report.startedAt = new Date().toISOString();
      const started = time.now();
      const now = () => time.now() - started;
      let readOrdinal = 0;
      let turnOrdinal = 0;

      const runAttempt = async (kind, ordinal, scheduledAt, operation) => {
        const owner = ordinal % clients.length;
        const attempt = { kind, ordinal, owner, scheduledMs: scheduledAt - started, startedMs: now() };
        report.attempts.push(attempt);
        try {
          await operation(clients[owner], attempt);
          attempt.status = 'PASS';
        } catch (error) {
          Object.assign(attempt, safeFailure(error));
          throw error;
        } finally {
          attempt.finishedMs = now();
        }
      };
      const run = startLoad({
        ...connectedWorkload, durationMs, time, signal,
        onSample: (sample) => report.samples.push(sample),
        read: ({ scheduledAt, signal }) => runAttempt('read', readOrdinal++, scheduledAt, async (client) => {
          const requestSignal = AbortSignal.any([signal, AbortSignal.timeout(readTimeoutMs)]);
          const value = await client.json(client.agentPath, requestSignal);
          check(value?.id === client.fixture.agent && value.status === 'ready'
            && value.sandbox?.id === client.fixture.sandbox, 'local_workload_read_identity');
        }),
        turn: ({ scheduledAt, signal }) => runAttempt('turn', turnOrdinal++, scheduledAt, async (client, attempt) => {
          const requestSignal = AbortSignal.any([signal, AbortSignal.timeout(turnTimeoutMs)]);
          const key = `scale-${hash(`${report.runHash}:${attempt.ordinal}`)}`;
          attempt.keyHash = hash(key);
          await submitTurn(client, attempt, key, requestSignal, now, async (sequence, signal) => {
            const identity = `${attempt.owner}:${sequence}`;
            while (!terminals.has(identity)) await sleep(20, undefined, { signal });
            check(terminals.get(identity) === 'turn_completed', 'local_workload_agent_turn_failed');
          });
        }),
      });
      await run.done;
      signal.throwIfAborted();
      score(report);
    }, { ...dependencies, onEvent: (owner, event) => {
      const kind = event.kind;
      if (['turn_completed', 'turn_failed', 'turn_cancelled'].includes(kind?.type)) {
        check(Number.isSafeInteger(kind.turn_sequence) && kind.turn_sequence > 0, 'local_workload_agent_terminal_invalid');
        const identity = `${owner}:${kind.turn_sequence}`;
        check(!terminals.has(identity), 'local_workload_duplicate_terminal');
        terminals.set(identity, kind.type);
      }
    } });

    report.status = 'PASS';
  } catch (error) {
    Object.assign(report, safeFailure(error));
  } finally {
    report.finishedAt = new Date().toISOString();
  }
  return report;
}
