import type { ResolvedConfig } from './config.js';
import { ApiError, ProtocolError } from './errors.js';
import type { OperationOptions } from './http.js';
import { command, input, query, read, segment, type CommandOptions } from './resources.js';
import { resourceSelector, shareMode, shareReceipt } from './resource-access.js';
import { revocationStatusTarget } from './resource-revocation-links.js';
import {
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  positiveInteger,
  tagged,
  text,
  type Output,
} from './schema.js';

export const revocationRequest = object({
  kind: enumeration('agent', 'sandbox'),
  id: text,
  share_id: text,
  expected_revision: positiveInteger,
  change: tagged(
    {
      revoke: object({ kind: enumeration('revoke') }),
      set_mode: object({ kind: enumeration('set_mode'), mode: shareMode }),
    },
    'kind',
  ),
});
export type RevocationRequest = Output<typeof revocationRequest>;
const statusShape = object({
  operation_id: text,
  selector: resourceSelector,
  share_id: text,
  expected_revision: positiveInteger,
  state: enumeration('pending', 'evidence_received', 'effective'),
  participants: integer,
  stopped: integer,
  unresolved_effects: integer,
  deadline_unix_ms: positiveInteger,
  deadline_exceeded: boolean,
  receipt: nullable(shareReceipt),
});
export type ResourceRevocationStatus = Output<typeof statusShape>;

export function revocationStatus(raw: unknown): ResourceRevocationStatus {
  const value = statusShape(raw);
  if (
    !/^[a-f0-9]{64}$/.test(value.operation_id) ||
    value.participants < 0 ||
    value.participants > 64 ||
    value.stopped < 0 ||
    value.stopped > value.participants ||
    value.unresolved_effects < 0 ||
    value.unresolved_effects > value.participants * 128 ||
    (value.state === 'effective') !== (value.receipt !== null) ||
    (value.state !== 'pending' && value.stopped !== value.participants) ||
    (value.receipt !== null &&
      (value.receipt.operation_id !== value.operation_id ||
        value.receipt.share.id !== value.share_id ||
        value.receipt.share.selector.kind !== value.selector.kind ||
        ('agent_id' in value.selector
          ? !('agent_id' in value.receipt.share.selector) ||
            value.receipt.share.selector.agent_id !== value.selector.agent_id
          : !('sandbox_id' in value.receipt.share.selector) ||
            value.receipt.share.selector.sandbox_id !== value.selector.sandbox_id) ||
        value.receipt.share.revision !== value.expected_revision + 1))
  ) {
    throw new ProtocolError('Revocation status contains inconsistent evidence or identity');
  }
  return value;
}

/** Return only a validated original operation ID; never follow arbitrary links. */
export function pendingRevocation(
  error: unknown,
  kind: 'agent' | 'sandbox',
  id: string,
): string | undefined {
  if (
    !(error instanceof ApiError) ||
    error.status !== 409 ||
    error.code !== 'resource_revocation_pending'
  )
    return undefined;
  const match = /^\/api\/v1\/resource-revocations\/([a-f0-9]{64})(\?[^<>\s#]*)$/.exec(
    revocationStatusTarget(error.statusLink) ?? '',
  );
  if (!match) return undefined;
  const params = new URLSearchParams(match[2]);
  if (params.size !== 2 || params.get('kind') !== kind || params.get('id') !== id) return undefined;
  return match[1];
}

function matching(raw: unknown, kind: 'agent' | 'sandbox', id: string, operation?: string) {
  const value = revocationStatus(raw);
  const target =
    value.selector.kind === 'agent' ? value.selector.agent_id : value.selector.sandbox_id;
  if (
    value.selector.kind !== kind ||
    target !== id ||
    (operation !== undefined && operation !== value.operation_id)
  )
    throw new ProtocolError('Revocation status does not match the requested resource');
  return value;
}

export class ResourceRevocations {
  constructor(private readonly config: ResolvedConfig) {}
  async request(body: RevocationRequest, options: CommandOptions) {
    const value = await read(
      this.config,
      '/api/v1/resource-revocations',
      (raw) => matching(raw, body.kind, body.id),
      {
        ...command(options),
        method: 'POST',
        body: input(revocationRequest, body),
        expectedStatus: 202,
      },
    );
    if (value.share_id !== body.share_id || value.expected_revision !== body.expected_revision)
      throw new ProtocolError('Revocation status does not match the original share revision');
    if (
      value.receipt !== null &&
      (body.change.kind === 'revoke'
        ? value.receipt.share.revoked_at === null
        : value.receipt.share.revoked_at !== null || value.receipt.share.mode !== body.change.mode)
    )
      throw new ProtocolError('Revocation receipt does not confirm the requested change');
    return value;
  }
  status(operation: string, kind: 'agent' | 'sandbox', id: string, options: OperationOptions = {}) {
    return read(
      this.config,
      query(`/api/v1/resource-revocations/${segment(operation)}`, { kind, id }),
      (raw) => matching(raw, kind, id, operation),
      options,
    );
  }
}
