import {isDeepStrictEqual} from 'node:util';
import {check} from '../scale-tests/lib/checks.mjs';

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
export function runtimeStorage(spec, binding) {
  const expectedVolumes = [{name: 'home', persistentVolumeClaim: {claimName: binding.pvcs[0].name, readOnly: false}}];
  check(isDeepStrictEqual(spec?.volumes, expectedVolumes), 'native_retained_volumes_changed');
  const runtime = spec.containers.find(container => container.name === 'runtime');
  const mounts = [
    {name: 'home', mountPath: '/home/cma', subPath: 'home', readOnly: false},
    {name: 'home', mountPath: '/var/lib/cma-runtime', subPath: 'state', readOnly: false},
    {name: 'home', mountPath: '/var/lib/cma-transfer', subPath: 'transfer', readOnly: false},
  ];
  const ordered = list => Array.isArray(list) ? [...list].sort((a, b) =>
    String(a?.mountPath).localeCompare(String(b?.mountPath))) : list;
  check(isDeepStrictEqual(ordered(runtime?.volumeMounts), ordered(mounts))
    && (runtime.volumeDevices === undefined || isDeepStrictEqual(runtime.volumeDevices, [])),
  'native_runtime_mounts_changed');
}
