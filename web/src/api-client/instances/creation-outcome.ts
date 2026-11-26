import { ProtocolError } from '../errors.js';
import { enumeration, integer, object, optional, tagged, text, type Output } from '../schema.js';

const version = (value: unknown): 1 => {
  if (integer(value) !== 1) throw new ProtocolError('unsupported creation evidence version');
  return 1;
};

export const creationOutcomeRequest = object({
  version,
  deployment: text,
  method: enumeration('POST'),
  path: enumeration('/api/v1/sandboxes', '/api/v1/agents'),
  idempotency_key: text,
  fingerprint: object({
    algorithm: enumeration('nyxid-wire-v1', 'sha256-wire-v1'),
    digest: text,
  }),
  claim_id: optional(text),
  operation_id: optional(text),
});

export type CreationOutcomeRequest = Output<typeof creationOutcomeRequest>;

const outcome = tagged(
  {
    created: object({
      kind: enumeration('created'),
      sandbox_id: text,
      operation_id: optional(text),
      proof_id: text,
    }),
    refused: object({
      kind: enumeration('refused'),
      code: enumeration('sandbox_limit_reached', 'trust_confirmation_required'),
      operation_id: optional(text),
      proof_id: text,
    }),
    never_received: object({ kind: enumeration('never_received'), proof_id: text }),
    in_progress: object({ kind: enumeration('in_progress'), operation_id: optional(text) }),
    unknown: object({
      kind: enumeration('unknown'),
      reason: enumeration(
        'no_durable_evidence',
        'legacy_fingerprint_unavailable',
        'generation_ambiguous',
        'recovery_required',
        'coverage_unavailable',
      ),
    }),
  },
  'kind',
);

export const creationOutcome = object({
  version,
  deployment: text,
  operation: enumeration('sandbox', 'agent_bootstrap'),
  identity_digest: text,
  result: outcome,
});
