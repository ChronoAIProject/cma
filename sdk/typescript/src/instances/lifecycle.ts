import {
  boolean,
  enumeration,
  object,
  optional,
  positiveInteger,
  tagged,
  text,
  type Output,
} from '../schema.js';

const pausePhase = enumeration(
  'capture_and_publish_checkpoint',
  'pause_agents',
  'teardown_runtime',
  'publish_paused',
  'resume_writers',
);
const revivePhases = [
  'checkpoint_verification',
  'runtime_provisioning',
  'credential_delivery',
  'home_restore',
  'conversation_reopen',
  'queued_message_delivery',
  'publish_active',
] as const;
const revivePhase = enumeration(...revivePhases);
const rotationPhase = enumeration(
  'capture_and_publish_checkpoint',
  'pause_agents',
  'teardown_runtime',
  ...revivePhases,
);

const eligibility = object({
  allowed: boolean,
  reason: optional(
    enumeration('not_active', 'not_paused', 'checkpoint_in_progress', 'work_in_progress'),
  ),
});
export type SandboxLifecycleEligibility = Output<typeof eligibility>;
const checkpoint = object({
  phase: enumeration('capture', 'resume_writers'),
  started_at: text,
  epoch: positiveInteger,
});
export type SandboxCheckpoint = Output<typeof checkpoint>;
const operation = tagged(
  {
    pause: object({ purpose: enumeration('pause'), phase: pausePhase, started_at: text }),
    revive: object({ purpose: enumeration('revive'), phase: revivePhase, started_at: text }),
    image_rotation: object({
      purpose: enumeration('image_rotation'),
      phase: rotationPhase,
      started_at: text,
    }),
  },
  'purpose',
);
export type SandboxLifecycleOperation = Output<typeof operation>;
export const sandboxLifecycle = object({
  pause: eligibility,
  revive: eligibility,
  checkpoint: optional(checkpoint),
  background_save: optional(
    object({
      phase: enumeration(
        'capturing',
        'preparing_export',
        'exporting',
        'uploading',
        'finishing',
        'saved',
        'cancelled',
      ),
      started_at: text,
      captured_at: optional(text),
      epoch: positiveInteger,
    }),
  ),
  operation: optional(operation),
});
export type SandboxLifecycle = Output<typeof sandboxLifecycle>;
