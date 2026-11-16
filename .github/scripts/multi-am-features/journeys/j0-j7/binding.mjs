import {need, SHA256, canonical, digest} from './provenance.mjs';

const SHA = /^[a-f0-9]{40}$/;
const IMAGE = /@sha256:[a-f0-9]{64}$/;
export function localOrigin(value) {
  let url;
  try { url = new URL(value); } catch { need(false, 'mfunc01_origin_invalid'); }
  need(url.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)
    && !url.username && !url.password && url.pathname === '/' && !url.search && !url.hash,
  'mfunc01_nonlocal_origin');
  return url.origin;
}

export function validateLocalKindBinding(binding, release) {
  need(binding?.scope === 'LOCAL_DISPOSABLE_TWO_AM', 'mfunc01_scope_not_local_kind');
  need(binding.database_disposable === true && binding.coordination_enabled === true,
    'mfunc01_shared_disposable_runtime_required');
  need(binding.production_acceptance === false && binding.production_writes === false,
    'mfunc01_production_transport_refused');
  need(/^kind-[a-z0-9-]+$/.test(binding.kind_context ?? '') && binding.namespace,
    'mfunc01_kind_context_required');
  need(SHA.test(binding.api_source ?? '') && IMAGE.test(binding.api_image ?? ''),
    'mfunc01_exact_13b_release_required');
  need(release?.source === binding.api_source && release?.image === binding.api_image
    && release?.kind_context === binding.kind_context && release?.namespace === binding.namespace
    && SHA256.test(release.artifact_sha256 ?? ''), 'mfunc01_release_binding_mismatch');
  need(SHA.test(release.web?.source ?? '') && /^sha256:[a-f0-9]{64}$/.test(release.web?.manifest ?? '')
    && SHA256.test(release.web?.build_info_sha256 ?? ''), 'mfunc01_web_release_required');
  const origins = ['a', 'b'].map(am => localOrigin(binding.am_origins?.[am]));
  need(origins[0] !== origins[1], 'mfunc01_distinct_am_origins_required');
  localOrigin(binding.web_origin);
  return binding;
}

export async function observeBinding(binding, release, adapter, environment) {
  validateLocalKindBinding(binding, release);
  need(environment.source === release.source && environment.image === release.image,
    'mfunc01_runner_release_mismatch');
  need(typeof adapter?.topology === 'function', 'mfunc01_topology_observer_missing');
  const topology = await adapter.topology();
  need(topology?.kind_context === binding.kind_context && topology.namespace === binding.namespace
    && topology.shared_database === true && topology.coordination_enabled === true,
  'mfunc01_live_kind_scope_mismatch');
  for (const am of ['a', 'b']) {
    const member = topology.ams?.[am];
    need(member?.ready === true && member.image === release.image && member.source === release.source
      && typeof member.pod_uid === 'string' && member.pod_uid.length > 0
      && typeof member.instance_id === 'string' && member.instance_id.length > 0
      && localOrigin(member.origin) === localOrigin(binding.am_origins[am]),
    'mfunc01_two_ready_exact_release_required');
  }
  need(topology.ams.a.pod_uid !== topology.ams.b.pod_uid
    && topology.ams.a.instance_id !== topology.ams.b.instance_id,
  'mfunc01_distinct_live_processes_required');
  return {scope: binding.scope, source: release.source, image: release.image,
    artifact_sha256: release.artifact_sha256, kind_context: binding.kind_context,
    namespace: binding.namespace, topology_sha256: digest(canonical(topology)),
    ams: Object.fromEntries(['a', 'b'].map(am => [am, {
      pod_sha256: digest(topology.ams[am].pod_uid),
      instance_sha256: digest(topology.ams[am].instance_id), origin: localOrigin(binding.am_origins[am]),
    }]))};
}
