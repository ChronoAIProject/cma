import {isDeepStrictEqual} from 'node:util';
import {check, hash, isHash} from '../scale-tests/lib/checks.mjs';

const contracts = new Map([
  ['legacy-detached-v1', {native: false, fullHome: false}],
  ['native-vct-empty-v1', {native: true, fullHome: false}],
  ['native-vct-full-home-v2', {native: true, fullHome: true}],
]);
const identifier = value => typeof value === 'string' && /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(value);
const keys = (value, names) => value && isDeepStrictEqual(Object.keys(value).sort(), [...names].sort());

export function homeContract(binding) {
  const contract = contracts.get(binding.home_contract?.kind);
  check(contract, 'native_home_contract_unsupported');
  return contract;
}

export function validateHomeContract(binding) {
  const policy = binding.home_contract;
  if (!homeContract(binding).native) {
    check(keys(policy, ['kind']), 'native_home_contract_mixed');
    return;
  }
  check(keys(policy, ['kind', 'storage_class', 'capacity_gib', 'pool_config_sha256', 'claim'])
    && identifier(policy.storage_class) && Number.isSafeInteger(policy.capacity_gib)
    && policy.capacity_gib >= 1 && policy.capacity_gib <= 1024
    && isHash(policy.pool_config_sha256) && keys(policy.claim, ['name', 'uid', 'request_sha256', 'request_json_sha256'])
    && identifier(policy.claim.name) && identifier(policy.claim.uid)
    && isHash(policy.claim.request_sha256) && isHash(policy.claim.request_json_sha256)
    && binding.pvcs[0].name === `home-${binding.sandbox.name}`, 'native_vct_binding_invalid');
}

function storagePolicy(policy) {
  return {accessModes: ['ReadWriteOncePod'], volumeMode: 'Filesystem', storageClassName: policy.storage_class,
    resources: {requests: {storage: `${policy.capacity_gib}Gi`}}};
}

export function templateStorage(value, binding) {
  if (!homeContract(binding).native) {
    check(isDeepStrictEqual(value.spec.volumeClaimTemplates, []), 'native_retained_volumes_changed');
    return;
  }
  const policy = binding.home_contract;
  const expected = [{metadata: {name: 'home', labels: {'cma/pool-config': policy.pool_config_sha256.slice(0, 52)},
    annotations: {'cma/home-layout': policy.kind}}, spec: storagePolicy(policy)}];
  check(isDeepStrictEqual(value.spec.volumeClaimTemplates, expected), 'native_vct_template_changed');
}

export function nativeVolumePolicy(value, binding) {
  const policy = binding.home_contract;
  const {volumeName, volumeMode = 'Filesystem', ...spec} = value.spec;
  // Read the admitted policy, not current pool defaults; replacement or expansion
  // must never silently rewrite the original observation baseline.
  check(volumeName === binding.pvcs[0].volume
    && isDeepStrictEqual({...spec, volumeMode}, storagePolicy(policy))
    && value.metadata.labels?.['cma/pool-config'] === policy.pool_config_sha256.slice(0, 52)
    && value.metadata.annotations?.['cma/home-layout'] === policy.kind,
  'native_vct_storage_changed');
}

export function claimAuthority(value, binding) {
  const policy = binding.home_contract;
  const annotations = value.metadata.annotations;
  check(value.metadata.labels?.['cma/control-plane'] === binding.deployment
    && value.status?.sandbox?.name === binding.sandbox.name
    && annotations?.['cma/native-pool-config'] === policy.claim.request_sha256
    && typeof annotations?.['cma/pool-claim-request'] === 'string'
    && hash(annotations['cma/pool-claim-request']) === policy.claim.request_json_sha256
    && typeof annotations?.['cma/native-home-origin'] === 'string'
    && hash(annotations['cma/native-home-origin']) === binding.native_origin_sha256
    && isDeepStrictEqual(value.spec, {warmPoolRef: {name: `cma-pool-${policy.pool_config_sha256.slice(0, 48)}`}}),
  'native_claim_authority_changed');
}
