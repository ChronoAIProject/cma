import { boolean, enumeration, object, positiveInteger, text, type Output } from '../schema.js';

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

export const recoverRetainedRequest = object({
  expected_revision: positiveInteger,
  confirm_same_image_recovery: boolean,
});
export type RecoverRetainedRequest = Output<typeof recoverRetainedRequest>;

export const retainedReopenReceipt = object({
  operation_id: text,
  revision: positiveInteger,
  accepted_at: text,
});
export type RetainedReopenReceipt = Output<typeof retainedReopenReceipt>;
