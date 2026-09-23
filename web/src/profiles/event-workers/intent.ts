import type { EventWorkerProfileInput } from '@chronoai/cma-sdk';

export interface WorkerSaveIntent {
  readonly key: string;
  readonly body: EventWorkerProfileInput;
  readonly expectedRevision?: number;
}
const storageKey = (owner: string, id: string) =>
  `cma:event-worker-save:${encodeURIComponent(owner)}:${encodeURIComponent(id)}`;

export function readIntent(owner: string, id: string): WorkerSaveIntent | undefined {
  const raw = sessionStorage.getItem(storageKey(owner, id));
  if (raw === null) return undefined;
  const value = JSON.parse(raw) as WorkerSaveIntent;
  if (
    typeof value.key !== 'string' ||
    typeof value.body?.name !== 'string' ||
    typeof value.body.description !== 'string' ||
    !['agent_profile', 'workspace_profile', 'sandbox_profile'].every((kind) => {
      const ref = value.body[kind as 'agent_profile'];
      return typeof ref?.id === 'string' && Number.isInteger(ref.revision) && ref.revision > 0;
    }) ||
    (value.expectedRevision !== undefined &&
      (!Number.isInteger(value.expectedRevision) || value.expectedRevision < 1))
  ) {
    throw new Error('The retained save could not be read.');
  }
  return value;
}
export function retainIntent(owner: string, id: string, intent: WorkerSaveIntent) {
  sessionStorage.setItem(storageKey(owner, id), JSON.stringify(intent));
}
export function clearIntent(owner: string, id: string) {
  sessionStorage.removeItem(storageKey(owner, id));
}
