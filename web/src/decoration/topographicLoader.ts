import type { createTopographicSurface } from './vendor/reactdrops-topographic/topographic-surface';

type SurfaceFactory = typeof createTopographicSurface;
let loading: Promise<SurfaceFactory | null> | undefined;

export function loadTopographicSurface(): Promise<SurfaceFactory | null> {
  loading ??= import('./vendor/reactdrops-topographic/topographic-surface')
    .then((module) => module.createTopographicSurface)
    .catch(() => null);
  return loading;
}
