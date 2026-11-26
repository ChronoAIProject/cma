import { array, boolean, integer, nullable, object, optional, record, text } from '../schema.js';

const conversation = object({ agent_id: text, conversation_id: text, history_sha256: text });
const identity = {
  runtime_id: text,
  runtime_generation: integer,
  home_identity: text,
  boot_id: nullable(text),
  runtime_process_absent: boolean,
  provider_plan_sha256: text,
  conversations: array(conversation),
};

// These are recorded backend evidence, never instructions or authority for the
// browser. Preserve the wire shape while keeping provider details out of Activity.
export const runtimeRecoveryProof = object({
  ...identity,
  sandbox_id: text,
  reserved_provision: optional(object({ operation_id: text, generation: integer })),
  observed_provision_generation: optional(integer),
  requires_native_quiescence: boolean,
  provider_plan: record,
});

export const runtimeRecoveryReceipt = object({
  ...identity,
  operation_id: text,
  completed_at: text,
});
