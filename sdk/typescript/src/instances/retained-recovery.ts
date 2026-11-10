import { boolean, enumeration, object, type Output } from '../schema.js';

/** Read-only facts. No returned value is permission to interrupt or recover work. */
export const retainedRecoveryAssessment = object({
  disposition: enumeration(
    'active_lifecycle',
    'recorded_boot_present',
    'exact_boot_replacement',
    'exact_suspended_runtime',
    'evidence_unavailable',
    'authority_changed',
    'terminal_failure',
    'child_activity',
    'unsupported_image_authority',
    'provider_evidence_refused',
    'incomplete_history',
  ),
  admission_enabled: boolean,
});
export type RetainedRecoveryAssessment = Output<typeof retainedRecoveryAssessment>;
