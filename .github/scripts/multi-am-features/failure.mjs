import {createHash, randomUUID} from 'node:crypto';
import {writeFile} from 'node:fs/promises';
import {join} from 'node:path';

export const errorCode = error => /^[a-zA-Z0-9_:-]{1,160}$/.test(error?.message ?? '')
  ? error.message : 'transport_or_contract_error';

export function lifecycleFailureSummary(operation) {
  if (!operation) return null;
  const statuses = ['queued', 'running', 'ready', 'succeeded', 'failed', 'waiting_for_connection', 'cancelled'];
  const phases = ['queued', 'sandbox_ready', 'key_scope_ready', 'workspace_runtime', 'agent_startup', 'failed', 'cancelled'];
  return {
    status: statuses.includes(operation.status) ? operation.status : 'unknown',
    phase: phases.includes(operation.phase) ? operation.phase : 'unknown',
    agent_assigned: typeof operation.agent_id === 'string' && operation.agent_id.length > 0,
  };
}

export function journeyFailureSummary(journey) {
  const step = journey?.steps.find(value => value.verdict === 'FAIL');
  const cleanup = journey?.cleanup ?? [];
  const fixedCode = value => typeof value === 'string' && /^[a-z][a-z0-9_]{0,100}$/.test(value)
    ? value : 'transport_or_contract_error';
  const podCount = journey?.failure_capture?.runtime_pod_count;
  return {
    first_failed_step: step ? fixedCode(step.name) : null,
    first_failed_step_code: step ? fixedCode(step.error) : null,
    lifecycle_failure: journey?.create_failure?.observation ?? null,
    failure_runtime_pod_count: Number.isSafeInteger(podCount) && podCount >= 0 ? podCount : null,
    journey_cleanup_failures: {
      public_retirement: cleanup.filter(value => value.public_cleanup === 'FAILED').length,
      provider_release: cleanup.filter(value => value.provider_release === 'FAILED').length,
      manager_restore: cleanup.filter(value => value.am_restore === 'FAILED').length,
    },
  };
}

// Keep typed response bodies and correlation IDs private. The returned receipt
// contains only a classification and the hash of the complete private record.
export async function retainFailure(directory, stage, error, context = {}) {
  if (!['primary', 'restoration'].includes(stage)) throw Error('invalid_failure_stage');
  const record = {stage, context, error_code: errorCode(error), response: error?.response ?? null};
  const raw = JSON.stringify(record, null, 2) + '\n';
  await writeFile(join(directory, 'failure-' + randomUUID() + '.private.json'), raw,
    {flag: 'wx', mode: 0o600});
  const code = record.response?.body?.code;
  return {stage, error_code: record.error_code, status: record.response?.status,
    problem_code: typeof code === 'string' && /^[a-zA-Z0-9_:-]{1,160}$/.test(code) ? code : undefined,
    evidence_sha256: createHash('sha256').update(raw).digest('hex')};
}

export async function withRestoration(work, restore, recordFailure = async () => {}) {
  let primary, restoration, evidenceFailure, failed = false, restoreFailed = false;
  async function retain(stage, error) {
    try { await recordFailure(stage, error); }
    catch (failure) { evidenceFailure ??= failure; }
  }
  try { await work(); }
  catch (error) { primary = error; failed = true; await retain('primary', error); }
  try { await restore(); }
  catch (error) { restoration = error; restoreFailed = true; await retain('restoration', error); }
  if (failed) throw primary;
  if (restoreFailed) throw restoration;
  if (evidenceFailure) throw evidenceFailure;
}
