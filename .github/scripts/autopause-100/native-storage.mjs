import {isDeepStrictEqual} from 'node:util';
import {check} from '../scale-tests/lib/checks.mjs';
import {homeContract} from './native-contract.mjs';

// Kubernetes omits false optional readOnly values when serializing typed Pods.
// Only absence defaults to false; null and unknown fields remain mismatches.
const defaultReadOnly = value => value && {...value, readOnly: value.readOnly === undefined ? false : value.readOnly};
const orderedMounts = list => Array.isArray(list) ? list.map(defaultReadOnly).sort((a, b) =>
  String(a?.mountPath).localeCompare(String(b?.mountPath))) : list;

export function usesRetainedVolume(spec, binding) {
  check(spec && typeof spec === 'object', 'native_pod_volumes_invalid');
  const volumes = spec.volumes === undefined ? [] : spec.volumes;
  check(Array.isArray(volumes) && volumes.length <= 128 && volumes.every(volume => volume
    && typeof volume.name === 'string' && /^[a-z0-9][a-z0-9-]{0,62}$/.test(volume.name)
    && (volume.persistentVolumeClaim === undefined || (volume.persistentVolumeClaim
      && typeof volume.persistentVolumeClaim.claimName === 'string'
      && /^[a-z0-9][a-z0-9.-]{0,252}$/.test(volume.persistentVolumeClaim.claimName))))
    && new Set(volumes.map(volume => volume.name)).size === volumes.length,
  'native_pod_volumes_invalid');
  return volumes.some(volume => binding.pvcs.some(pvc => pvc.name === volume.persistentVolumeClaim?.claimName));
}

// pool/model.rs defines one home volume and three persistent runtime subtrees.
// Unknown additional mounts can shadow these paths, so they are not ignored.
export function runtimeStorage(spec, binding, template = false) {
  const contract = homeContract(binding);
  const expectedVolumes = template && contract.native ? []
    : [{name: 'home', persistentVolumeClaim: {claimName: binding.pvcs[0].name, readOnly: false}}];
  const rawVolumes = spec?.volumes === undefined ? [] : spec.volumes;
  const volumes = Array.isArray(rawVolumes) ? rawVolumes.map(volume => volume?.persistentVolumeClaim
    ? {...volume, persistentVolumeClaim: defaultReadOnly(volume.persistentVolumeClaim)} : volume) : rawVolumes;
  check(isDeepStrictEqual(volumes, expectedVolumes), 'native_retained_volumes_changed');
  check(spec.containers.length === 1
    && (spec.ephemeralContainers === undefined || isDeepStrictEqual(spec.ephemeralContainers, [])),
  'native_storage_containers_changed');
  const runtime = spec.containers.find(container => container.name === 'runtime');
  const mounts = [
    {name: 'home', mountPath: contract.fullHome ? '/home' : '/home/cma', subPath: 'home', readOnly: false},
    {name: 'home', mountPath: '/var/lib/cma-runtime', subPath: 'state', readOnly: false},
    {name: 'home', mountPath: '/var/lib/cma-transfer', subPath: 'transfer', readOnly: false},
  ];
  check(isDeepStrictEqual(orderedMounts(runtime?.volumeMounts), orderedMounts(mounts))
    && (runtime.volumeDevices === undefined || isDeepStrictEqual(runtime.volumeDevices, [])),
  'native_runtime_mounts_changed');
  const initializers = spec.initContainers === undefined ? [] : spec.initContainers;
  if (!contract.native) {
    check(isDeepStrictEqual(initializers, []), 'native_initializer_changed');
    return;
  }
  const init = initializers?.[0];
  check(Array.isArray(initializers) && initializers.length === 1 && init?.name === 'initialize-home'
    && init.image === binding.image && isDeepStrictEqual(init.command, ['/usr/local/bin/layer4-runtime',
      contract.fullHome ? '--initialize-home-tree' : '--initialize-home', '/mnt/cma-home'])
    && (init.args === undefined || isDeepStrictEqual(init.args, []))
    && (init.volumeDevices === undefined || isDeepStrictEqual(init.volumeDevices, []))
    && isDeepStrictEqual(orderedMounts(init.volumeMounts), [{name: 'home', mountPath: '/mnt/cma-home', readOnly: false}]),
  'native_initializer_changed');
}
