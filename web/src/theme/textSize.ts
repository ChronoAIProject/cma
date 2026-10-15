import { useSyncExternalStore } from 'react';

export type SystemTextSize = 'small' | 'medium' | 'large';
export const SYSTEM_TEXT_SIZES: readonly SystemTextSize[] = ['small', 'medium', 'large'];
const KEY = 'cma:system-text-size';
const EVENT = 'cma-system-text-size';
const SCALE: Record<SystemTextSize, string> = { small: '0.9', medium: '1', large: '1.15' };
let ephemeral: SystemTextSize | undefined;

function valid(value: unknown): value is SystemTextSize {
  return value === 'small' || value === 'medium' || value === 'large';
}
export function storedTextSize(): SystemTextSize {
  try {
    const value = localStorage.getItem(KEY);
    return valid(value) ? value : (ephemeral ?? 'medium');
  } catch {
    return ephemeral ?? 'medium';
  }
}
function apply(size: SystemTextSize) {
  document.documentElement.dataset.systemTextSize = size;
  document.documentElement.style.setProperty('--cma-system-font-scale', SCALE[size]);
}
export function setTextSize(size: SystemTextSize) {
  ephemeral = size;
  try {
    localStorage.setItem(KEY, size);
  } catch {
    /* Still apply when storage is blocked. */
  }
  apply(size);
  window.dispatchEvent(new Event(EVENT));
}
function subscribe(changed: () => void) {
  window.addEventListener(EVENT, changed);
  const storage = (event: StorageEvent) => {
    if (event.key === KEY || event.key === null) {
      ephemeral = undefined;
      apply(storedTextSize());
      changed();
    }
  };
  window.addEventListener('storage', storage);
  return () => {
    window.removeEventListener(EVENT, changed);
    window.removeEventListener('storage', storage);
  };
}
export function useSystemTextSize(): SystemTextSize {
  return useSyncExternalStore(subscribe, storedTextSize, () => 'medium');
}
export function startTextSize() {
  apply(storedTextSize());
}
export function textSizeLabel(size: SystemTextSize): string {
  return size.charAt(0).toUpperCase() + size.slice(1);
}
