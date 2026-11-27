import { readFile, realpath, stat } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
import { check, hash, isHash, reconcile } from './checks.mjs';
import { LocalHttp } from './http.mjs';
import { LocalCluster } from './kubernetes.mjs';
import { connectedWorkloadScenario, connectedWorkloadSelection } from './report.mjs';
import { validateConfig } from './safety.mjs';

const MAX_ARTIFACT_BYTES = 16 * 1024 * 1024;
const roles = ['admissions', 'authority', 'durable', 'receiver'];

async function readPinnedJson(binding, kind) {
  check(isAbsolute(binding?.path ?? '') && isHash(binding?.sha256),
    `oracle_${kind}_binding_missing`, true);
  const path = await realpath(binding.path);
  const info = await stat(path);
  check(path === binding.path && info.isFile() && (info.mode & 0o077) === 0
    && info.size > 0 && info.size <= MAX_ARTIFACT_BYTES,
  `oracle_${kind}_scope_invalid`, true);
  const bytes = await readFile(path);
  check(bytes.length <= MAX_ARTIFACT_BYTES && hash(bytes) === binding.sha256,
    `oracle_${kind}_changed`, true);
  return JSON.parse(bytes.toString('utf8'));
}

async function readCapture(binding, role, runHash) {
  const capture = await readPinnedJson(binding, 'capture');
  check(capture.schema === 'cma-local-outcome-capture-v1' && capture.role === role
    && capture.runHash === runHash && capture.complete === true
    && isHash(capture.sourceHash) && Array.isArray(capture.records),
  'oracle_capture_incomplete', true);
  return capture;
}

async function readConnectedWorkload(config) {
  const binding = config.outcome.workloadReport;
  const report = await readPinnedJson(binding, 'workload');
  const row = report?.scenarios?.[0];
  const evidence = row?.evidence;
  check(report?.schema === 'cma-scale-results-v1' && report.environment === 'local'
    && report.selection === connectedWorkloadSelection
    && Array.isArray(report.scenarios) && report.scenarios.length === 1
    && row?.scenario === connectedWorkloadScenario && row.status === 'PASS'
    && evidence?.schema === 'cma-local-workload-v1' && evidence.status === 'PASS'
    && row.evidence_hash === hash(JSON.stringify(evidence)) && evidence.runHash === config.outcome.runHash
    && evidence.fixtureHash === config.fixtureHash && evidence.workloadHash === config.workloadHash
    && typeof evidence.sourceSha === 'string' && hash(evidence.sourceSha) === config.sourceHash
    && Array.isArray(evidence.attempts), 'oracle_workload_report_mismatch', true);
  const turns = evidence.attempts.filter((event) => event.kind === 'turn');
  check(turns.length > 0 && turns.every(({ status, commandHash }) => status === 'PASS' && isHash(commandHash)),
    'oracle_workload_turn_missing', true);
  const receipts = new Set(turns.map(({ commandHash }) => commandHash));
  check(turns.length === receipts.size, 'oracle_workload_duplicate_command');
  return { receipts, sha256: binding.sha256 };
}

function commandRecords(records, accepted) {
  const byCommand = new Map();
  for (const record of records) {
    check(isHash(record.commandHash) && accepted.has(record.commandHash), 'oracle_unexpected_command');
    const rows = byCommand.get(record.commandHash) ?? [];
    rows.push(record);
    byCommand.set(record.commandHash, rows);
  }
  return byCommand;
}

function authority(record) {
  check(isHash(record.ownerHash) && Number.isSafeInteger(record.runtimeGeneration)
    && record.runtimeGeneration > 0 && Number.isSafeInteger(record.authorityEpoch)
    && record.authorityEpoch > 0, 'oracle_authority_missing', true);
  return [record.ownerHash, record.runtimeGeneration, record.authorityEpoch].join(':');
}

function observedSequence(record) {
  check(Number.isSafeInteger(record.observedSequence) && record.observedSequence >= 0,
    'oracle_observation_sequence_missing', true);
  return record.observedSequence;
}

// Each fixture observation uses the same independent ordering sequence.
// Authority intervals are captured by the ownership observer, including any
// takeover, independently of the execution and receiver observations.
function authorityTimeline(records) {
  check(records.length > 0, 'oracle_authority_timeline_missing', true);
  for (const row of records) {
    authority(row);
    check(Number.isSafeInteger(row.fromSequence) && row.fromSequence >= 0
      && Number.isSafeInteger(row.throughSequence) && row.throughSequence >= row.fromSequence,
    'oracle_authority_interval_invalid', true);
  }
  const ordered = [...records].sort((a, b) => a.fromSequence - b.fromSequence);
  for (let index = 1; index < ordered.length; index += 1) {
    const previous = ordered[index - 1];
    const current = ordered[index];
    check(current.fromSequence === previous.throughSequence + 1,
      'oracle_authority_interval_discontinuous', true);
    check(current.runtimeGeneration > previous.runtimeGeneration
      || (current.runtimeGeneration === previous.runtimeGeneration
        && current.authorityEpoch > previous.authorityEpoch),
    'oracle_authority_regressed', true);
  }
  return ordered;
}

function expectedAuthority(timeline, sequence, admittedAt) {
  check(sequence >= admittedAt, 'oracle_observation_before_admission');
  const expected = timeline.find((row) => row.fromSequence <= sequence && sequence <= row.throughSequence);
  check(expected, 'oracle_authority_at_observation_missing', true);
  return expected;
}

function durableWriteSequence(row, admittedAt) {
  const readAt = observedSequence(row);
  if (row.terminal !== true || !Object.hasOwn(row, 'terminalSequence')) return readAt;
  // The writer belongs to the durable transition, even if a later owner reads it.
  check(Number.isSafeInteger(row.terminalSequence) && row.terminalSequence > admittedAt
    && row.terminalSequence <= readAt, 'oracle_fault_terminal_sequence_missing', true);
  return row.terminalSequence;
}

function checkFaultLifetime(witness, executions, admittedAt) {
  const { injectionSequence, faultSequence, deathObservedSequence } = witness;
  check([injectionSequence, faultSequence, deathObservedSequence].every(Number.isSafeInteger)
    && admittedAt < injectionSequence && injectionSequence <= faultSequence && faultSequence <= deathObservedSequence,
  'oracle_fault_lifecycle_order_invalid');
  check(executions.length === 1 && executions[0].terminal === true, 'oracle_fault_terminal_missing', true);
  const terminal = executions[0];
  check(Object.hasOwn(terminal, 'terminalSequence'), 'oracle_fault_terminal_sequence_missing', true);
  check(faultSequence < durableWriteSequence(terminal, admittedAt), 'oracle_fault_command_already_terminal');
}

export async function readLocalOutcomeOracle({ runHash, captures }, { readCommand, workload, faultWitness } = {}) {
  check(isHash(runHash), 'oracle_run_binding_missing', true);
  check(captures && new Set(roles.map((role) => captures[role]?.path)).size === roles.length,
    'oracle_capture_source_reused', true);
  const [admissions, ownership, durable, receiver] = await Promise.all(
    roles.map((role) => readCapture(captures[role], role, runHash)));
  check(new Set([admissions.sourceHash, ownership.sourceHash, durable.sourceHash, receiver.sourceHash]).size === roles.length,
    'oracle_capture_source_reused', true);

  const accepted = new Map();
  for (const row of admissions.records) {
    check(isHash(row.commandHash) && !accepted.has(row.commandHash)
      && row.accepted === true && isHash(row.ownerHash), 'oracle_admission_invalid');
    accepted.set(row.commandHash, row);
  }
  check(accepted.size > 0, 'oracle_admission_missing', true);
  if (workload) {
    check(workload.receipts.size === accepted.size
      && [...accepted.keys()].every((commandHash) => workload.receipts.has(commandHash)),
    'oracle_workload_admissions_mismatch');
  }
  if (faultWitness) check(faultWitness.runHash === runHash && accepted.has(faultWitness.commandHash),
    'oracle_fault_command_mismatch');
  const ownershipByCommand = commandRecords(ownership.records, accepted);
  const recorded = commandRecords(durable.records, accepted);
  const effects = commandRecords(receiver.records, accepted);

  const commands = [];
  for (const [commandHash, admission] of accepted) {
    const admittedAt = observedSequence(admission);
    const timeline = authorityTimeline(ownershipByCommand.get(commandHash) ?? []);
    check(authority(admission) === authority(expectedAuthority(timeline, admittedAt, admittedAt)),
      'oracle_admission_authority_mismatch');
    const executions = recorded.get(commandHash) ?? [];
    if (faultWitness?.commandHash === commandHash) {
      checkFaultLifetime(faultWitness, executions, admittedAt);
      const atFault = expectedAuthority(timeline, faultWitness.faultSequence, admittedAt);
      check(authority(faultWitness) === authority(atFault) && isHash(atFault.processHash)
        && atFault.processHash === faultWitness.processHash, 'oracle_fault_authority_mismatch');
    }
    const observed = effects.get(commandHash) ?? [];
    const expectations = [
      ...executions.map((row) => ({ row, sequence: durableWriteSequence(row, admittedAt) })),
      ...observed.map((row) => ({ row, sequence: observedSequence(row) })),
    ].map(({ row, sequence }) => ({
      row, expected: expectedAuthority(timeline, sequence, admittedAt),
    }));
    const correctOwner = expectations.every(({ row, expected }) => row.ownerHash === expected.ownerHash);
    commands.push({ commandHash, accepted: true, executions: executions.length,
      effects: observed.length,
      staleWrites: expectations.filter(({ row, expected }) => authority(row) !== authority(expected)).length,
      terminal: executions.every((row) => row.terminal === true) && observed.every((row) => row.terminal === true),
      correctOwner });
    if (readCommand) {
      check(typeof admission.commandId === 'string' && hash(admission.commandId) === commandHash,
        'oracle_command_binding_missing', true);
      const live = await readCommand(admission.commandId);
      check(live?.receipt?.command_id === admission.commandId
        && (live.status === 'completed' || live.status === 'cancelled'),
      'oracle_live_terminal_mismatch');
    }
  }
  const result = { independent: true, complete: true, acceptedHashes: [...accepted.keys()], commands,
    evidenceHash: hash(JSON.stringify([runHash, ...roles.map((role) => captures[role].sha256), ...(workload ? [workload.sha256] : []), ...(faultWitness ? [faultWitness] : [])])) };
  reconcile(result);
  return result;
}

// Read-only driver for a real, isolated local fixture. The independent receiver
// and durable captures must have been recorded before the oracle is invoked.
export async function verifyLocalOutcomes(config, { fetchImpl, command, expectedWorkload, readCommand, faultWitness } = {}) {
  validateConfig(config);
  check(config.outcome?.origin === config.endpoints.am && config.outcome.cluster,
    'oracle_local_fixture_missing', true);
  const cluster = new LocalCluster({ ...config.outcome.cluster, context: config.context,
    namespace: config.namespace }, command);
  await cluster.verify();
  const http = new LocalHttp(config.outcome.origin, {
    bearer: process.env.CMA_LOCAL_ORACLE_BEARER, fetchImpl,
  });
  const workload = expectedWorkload ?? (config.outcome.workloadReport ? await readConnectedWorkload(config) : undefined);
  return readLocalOutcomeOracle(config.outcome, {
    workload, faultWitness,
    readCommand: readCommand ?? (async (id) => (await http.command(id)).value),
  });
}
