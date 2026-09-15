import { ProtocolError } from '../errors.js';
import { enumeration, object, positiveInteger, tagged, text, type Output } from '../schema.js';

export const sandboxCommandKind = enumeration('pause', 'revive', 'retire');
export const sandboxCommandRequest = object({
  command: sandboxCommandKind,
  expected_revision: positiveInteger,
});
const state = tagged(
  {
    queued: object({ status: enumeration('queued') }),
    applied: object({
      status: enumeration('applied'),
      at: text,
      sandbox_revision: positiveInteger,
      disposition: enumeration('scheduled', 'already_scheduled', 'already_satisfied'),
    }),
    rejected: object({
      status: enumeration('rejected'),
      at: text,
      reason: enumeration(
        'authority_revoked',
        'queue_expired',
        'target_changed',
        'not_eligible',
        'connections_required',
        'credential_setup_required',
      ),
    }),
  },
  'status',
);
const receipt = object({
  command_id: text,
  sandbox_id: text,
  command: sandboxCommandKind,
  expected_revision: positiveInteger,
  admitted_at: text,
  queue_deadline: text,
  revision: positiveInteger,
  state,
});

/** Applied records a committed transition; Sandbox status reports runtime progress. */
export type SandboxCommandReceipt = Output<typeof receipt>;
export type SandboxCommandRequest = Output<typeof sandboxCommandRequest>;
export type SandboxCommandKind = Output<typeof sandboxCommandKind>;
export type SandboxCommandState = Output<typeof state>;

export function sandboxCommandReceipt(value: unknown): SandboxCommandReceipt {
  const result = receipt(value);
  if (
    result.command_id === '' ||
    result.sandbox_id === '' ||
    result.revision !== (result.state.status === 'queued' ? 1 : 2)
  )
    throw new ProtocolError('Sandbox command identity or state revision is invalid');
  if (
    result.state.status === 'applied' &&
    result.state.sandbox_revision !==
      result.expected_revision + (result.state.disposition === 'scheduled' ? 1 : 0)
  )
    throw new ProtocolError('Sandbox command outcome revision is inconsistent');
  return result;
}
