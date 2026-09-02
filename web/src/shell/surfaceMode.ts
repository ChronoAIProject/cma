/** Which owner-facing session collection the shell is browsing. */
export type SurfaceMode = 'chat' | 'agent';

export const SURFACE_MODE_KEY = 'cma:surface-mode';

function isSurfaceMode(value: string | null): value is SurfaceMode {
  return value === 'chat' || value === 'agent';
}

/** Read the persisted surface, treating storage and untrusted values as Agent. */
export function loadSurfaceMode(): SurfaceMode {
  try {
    const value = window.localStorage.getItem(SURFACE_MODE_KEY);
    return isSurfaceMode(value) ? value : 'agent';
  } catch {
    return 'agent';
  }
}

/** Remember a valid surface without making storage availability a UI dependency. */
export function storeSurfaceMode(mode: SurfaceMode): void {
  try {
    window.localStorage.setItem(SURFACE_MODE_KEY, mode);
  } catch {
    // A blocked or full store loses a preference; the switch still works.
  }
}
