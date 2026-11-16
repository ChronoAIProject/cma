import {readFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {coverageCounts} from './counts.mjs';
const check = (value, code) => {if (!value) throw Error(code);};
export const receiptHash = value => createHash('sha256').update(JSON.stringify(value)).digest('hex');
export function registryFingerprint(registry) {
  return receiptHash({catalog: registry.catalog, definitions: [...registry.cases.values()]
    .map(({id, scope, evidence_scope, fixtures, steps}) => ({id, scope, evidence_scope, fixtures, steps}))
    .sort((a, b) => a.id.localeCompare(b.id))});
}
export function validateCleanupReceipt(config, binding, receipt, sha256, lastCaseFinishedAt) {
  check(receipt?.schema === 'multi-am-parent-cleanup-v1'
    && receipt.campaign_id === binding.campaign_id && receipt.source === binding.source && receipt.image === binding.image
    && receipt.scope === 'LOCAL_DISPOSABLE_TWO_AM'
    && /^[a-f0-9]{64}$/.test(config.disposable_database_sha256 ?? '')
    && receipt.database_sha256 === config.disposable_database_sha256
    && receipt.database_disposed === true && receipt.owned_children === 0 && receipt.status === 'PASS'
    && Number.isFinite(Date.parse(receipt.completed_at)) && Date.parse(receipt.completed_at) >= lastCaseFinishedAt
    && /^[a-f0-9]{64}$/.test(receipt.disposal_evidence_sha256 ?? '') && receiptHash(receipt) === sha256,
  'coverage_parent_cleanup_binding_invalid');
  check(!binding.final_cleanup || binding.final_cleanup.evidence_sha256 === sha256,
    'coverage_parent_cleanup_confirmation_changed');
  return {verified: true, evidence_sha256: sha256};
}
async function artifact(reference) {
  check(typeof reference?.path === 'string' && /^[a-f0-9]{64}$/.test(reference.sha256 ?? ''), 'finalization_artifact_reference');
  const bytes = await readFile(reference.path);
  check(createHash('sha256').update(bytes).digest('hex') === reference.sha256, 'finalization_artifact_hash_mismatch');
  return JSON.parse(bytes);
}

// Evidence-only: imports and this function start no application, browser or
// database process. The caller writes the returned additive packet separately.
export async function postDisposalFinalize({packetArtifact, cleanupArtifact, disposalArtifact, config, registry}) {
  const [packet, cleanup, disposal] = await Promise.all([artifact(packetArtifact), artifact(cleanupArtifact), artifact(disposalArtifact)]);
  const binding = structuredClone(packet.binding), results = packet.results;
  check(packet.kind === 'executable_private_packet' && binding?.source === config.expected_api_source
    && binding.image === config.expected_api_image && binding.campaign_id === config.run_id
    && binding.disposable_database_sha256 === config.disposable_database_sha256
    && binding.registry_sha256 === registryFingerprint(registry)
    && Number.isFinite(packet.last_case_finished_at) && Array.isArray(results)
    && results.every(result => result.source === binding.source && result.image === binding.image
      && result.campaign_id === binding.campaign_id && registry.cases.has(result.id)), 'finalization_saved_packet_binding');
  check(cleanup.disposal_evidence_sha256 === disposalArtifact.sha256
    && disposal.schema === 'multi-am-database-disposal-v1'
    && disposal.campaign_id === binding.campaign_id && disposal.database_sha256 === config.disposable_database_sha256
    && disposal.database_disposed === true && disposal.status === 'PASS' && disposal.owned_children === 0
    && Date.parse(disposal.completed_at) >= packet.last_case_finished_at
    && Date.parse(cleanup.completed_at) >= Date.parse(disposal.completed_at), 'finalization_disposal_evidence_binding');
  binding.final_cleanup = validateCleanupReceipt(config, binding, cleanup, receiptHash(cleanup), packet.last_case_finished_at);
  const counts = coverageCounts(registry, results, binding);
  const complete = Object.values(counts.roles).every(role => role.passing === role.total);
  return {schema: 'multi-am-post-disposal-finalization-v1', at: new Date().toISOString(),
    original_packet: packetArtifact, cleanup_artifact: cleanupArtifact, disposal_artifact: disposalArtifact,
    binding, results, counts, verdict: results.some(result => result.status === 'FAIL') ? 'FAIL' : complete ? 'PASS' : 'INCOMPLETE',
    runtime_acceptance: complete, application_processes_started: 0};
}
