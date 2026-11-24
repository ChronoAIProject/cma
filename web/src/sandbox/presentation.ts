import type { SandboxLifecycle, SandboxStatus } from '../api/types';
import { statusLabel, statusTone } from '../vocabulary/status';
import {
  sandboxHasParkedRuntimeUpdate,
  type SandboxRuntimeObservation,
} from './parkedRuntimeUpdate';

type Purpose = NonNullable<SandboxLifecycle['operation']>['purpose'];

export type SandboxStatusObservation = SandboxRuntimeObservation;

const operationStatus = {
  pause: 'pausing',
  revive: 'reviving',
  image_rotation: 'rotating',
} as const satisfies Record<Purpose, SandboxStatus>;

const groups = {
  active: 'live',
  paused: 'paused',
  failed: 'needs_attention',
  provisioning: 'changing',
  installing: 'changing',
  pausing: 'changing',
  reviving: 'changing',
  credential_rotating: 'changing',
  rotating: 'changing',
  retiring: 'deleting',
  retired: 'hidden',
} as const satisfies Record<SandboxStatus, string>;

function effectiveStatus(
  observation: SandboxStatusObservation,
  deletionRequested: boolean,
): SandboxStatus {
  const { status, lifecycle } = observation;
  if (status === 'retired') return status;
  if (status === 'retiring' || deletionRequested) return 'retiring';
  if (status === 'failed') return status;
  // The current operation can precede its raw status transition. A previous
  // outcome or a queued runtime update does not describe an operation in flight.
  const purpose = lifecycle?.operation?.purpose;
  return purpose === undefined ? status : operationStatus[purpose];
}

/** Apply the same status word, tone and inventory group to the facts a caller can see. */
export function sandboxStatusPresentation(
  observation: SandboxStatusObservation,
  deletionRequested = false,
) {
  const status = effectiveStatus(observation, deletionRequested);
  if (!deletionRequested && sandboxHasParkedRuntimeUpdate(observation))
    return {
      status,
      group: 'needs_attention' as const,
      label: 'Runtime update needs attention',
      tone: 'error' as const,
    };
  return { status, group: groups[status], label: statusLabel(status), tone: statusTone(status) };
}
