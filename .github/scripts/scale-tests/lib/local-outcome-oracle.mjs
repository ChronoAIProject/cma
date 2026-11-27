import { readFile, realpath, stat } from 'node:fs/promises';
import { isAbsolute } from 'node:path';
import { check, hash, isHash, reconcile } from './checks.mjs';
import { LocalHttp } from './http.mjs';
import { LocalCluster } from './kubernetes.mjs';
import { validateConfig } from './safety.mjs';

const MAX_ARTIFACT_BYTES = 16 * 1024 * 1024;
const roles = ['admissions', 'durable', 'receiver'];

async function readCapture(binding, role, runHash) {
  check(isAbsolute(binding?.path ?? '') && isHash(binding?.sha256),
    'oracle_capture_binding_missing', true);
  const path = await realpath(binding.path);
  const info = await stat(path);
  check(path === binding.path && info.isFile() && (info.mode & 0o077) === 0
    && info.size > 0 && info.size <= MAX_ARTIFACT_BYTES,
  'oracle_capture_scope_invalid', true);
  const bytes = await readFile(path);
  check(bytes.length <= MAX_ARTIFACT_BYTES && hash(bytes) === binding.sha256,
    'oracle_capture_changed', true);
  const capture = JSON.parse(bytes.toString('utf8'));
  check(capture.schema === 'cma-local-outcome-capture-v1' && capture.role === role
    && capture.runHash === runHash && capture.complete === true
    && isHash(capture.sourceHash) && Array.isArray(capture.records),
  'oracle_capture_incomplete', true);
  return capture;
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

// The three observations are produced by the admission client, the durable
// ledger reader, and the runtime/provider receiver respectively. A command
// result is not itself evidence that the receiver performed one effect.
export async function readLocalOutcomeOracle({ runHash, captures }, { readCommand } = {}) {
  check(isHash(runHash), 'oracle_run_binding_missing', true);
  check(captures && new Set(roles.map((role) => captures[role]?.path)).size === roles.length,
    'oracle_capture_source_reused', true);
  const [admissions, durable, receiver] = await Promise.all(
    roles.map((role) => readCapture(captures[role], role, runHash)));
  check(new Set([admissions.sourceHash, durable.sourceHash, receiver.sourceHash]).size === roles.length,
    'oracle_capture_source_reused', true);

  const accepted = new Map();
  for (const row of admissions.records) {
    check(isHash(row.commandHash) && !accepted.has(row.commandHash)
      && row.accepted === true && isHash(row.ownerHash), 'oracle_admission_invalid');
    accepted.set(row.commandHash, row);
  }
  check(accepted.size > 0, 'oracle_admission_missing', true);
  const recorded = commandRecords(durable.records, accepted);
  const effects = commandRecords(receiver.records, accepted);

  const commands = [];
  for (const [commandHash, admission] of accepted) {
    const executions = recorded.get(commandHash) ?? [];
    const observed = effects.get(commandHash) ?? [];
    const execution = executions[0];
    const expected = execution ? authority(execution) : null;
    const effectAuthorities = observed.map(authority);
    const correctOwner = execution?.ownerHash === admission.ownerHash
      && observed.every((row) => row.ownerHash === admission.ownerHash);
    commands.push({ commandHash, accepted: true, executions: executions.length,
      effects: observed.length, staleWrites: effectAuthorities.filter((value) => value !== expected).length,
      terminal: execution?.terminal === true && observed.every((row) => row.terminal === true),
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
    evidenceHash: hash(JSON.stringify([runHash, ...roles.map((role) => captures[role].sha256)])) };
  reconcile(result);
  return result;
}

// Read-only driver for a real, isolated local fixture. The independent receiver
// and durable captures must have been recorded before the oracle is invoked.
export async function verifyLocalOutcomes(config, { fetchImpl, command } = {}) {
  validateConfig(config);
  check(config.outcome?.origin === config.endpoints.am && config.outcome.cluster,
    'oracle_local_fixture_missing', true);
  const cluster = new LocalCluster({ ...config.outcome.cluster, context: config.context,
    namespace: config.namespace }, command);
  await cluster.verify();
  const http = new LocalHttp(config.outcome.origin, {
    bearer: process.env.CMA_LOCAL_ORACLE_BEARER, fetchImpl,
  });
  return readLocalOutcomeOracle(config.outcome, {
    readCommand: async (id) => (await http.command(id)).value,
  });
}
