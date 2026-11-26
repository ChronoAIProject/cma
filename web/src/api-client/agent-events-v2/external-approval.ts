import { ProtocolError } from '../errors.js';
import { enumeration, nullable, positiveInteger, tagged, text, type Output } from '../schema.js';
import { strict } from './schema.js';

export const externalApprovalPreference = 'cma-external-approval-v1';
export const approvalReviewPath = '/assistant/approvals';

export interface ExternalApprovalProjection {
  readonly supported: boolean;
  readonly reviewOrigin?: string;
  readonly canReview: boolean;
}

/** Policy comes from this authenticated response, never from an event URL. */
export function externalApprovalProjection(
  headers: Headers,
  requested: boolean,
): ExternalApprovalProjection {
  const supported =
    requested &&
    (headers.get('Preference-Applied') ?? '')
      .split(',')
      .some((token) => token.trim() === externalApprovalPreference);
  if (!supported) return { supported: false, canReview: false };
  const source = headers.get('CMA-Approval-Review-Origin');
  let reviewOrigin: string | undefined;
  try {
    const url = new URL(source ?? '');
    if (url.protocol === 'https:' && url.origin === source) reviewOrigin = source;
  } catch {
    /* Missing or invalid policy never authorizes a destination. */
  }
  return {
    supported: true,
    reviewOrigin,
    canReview:
      reviewOrigin !== undefined && headers.get('CMA-External-Approval-Action') === 'review',
  };
}

function boundedText(value: unknown, maximum: number) {
  const result = text(value);
  if (
    !result.trim() ||
    new TextEncoder().encode(result).length > maximum ||
    Array.from(result).some((character) => {
      const code = character.codePointAt(0)!;
      return code < 32 || (code >= 127 && code <= 159);
    })
  )
    throw new ProtocolError('External approval value exceeds its text boundary');
  return result;
}
function timestamp(value: unknown) {
  const result = text(value);
  const time = Date.parse(result);
  if (!Number.isFinite(time) || new Date(time).toISOString() !== result)
    throw new ProtocolError('External approval time is not canonical UTC');
  return result;
}
const identity = {
  approval_id: (value: unknown) => boundedText(value, 128),
  provider: enumeration('nyxid'),
  turn_sequence: positiveInteger,
  revision: positiveInteger,
};
const kinds = tagged({
  external_approval_requested: strict({
    type: enumeration('external_approval_requested'),
    ...identity,
    waiting_since: timestamp,
    deadline_at: timestamp,
    approve_url: nullable((value: unknown) => boundedText(value, 2048)),
  }),
  external_approval_resolved: strict({
    type: enumeration('external_approval_resolved'),
    ...identity,
    resolution: enumeration(
      'approved',
      'denied',
      'expired',
      'disconnected',
      'timed_out',
      'cancelled',
    ),
  }),
});
export type ExternalApprovalKind = Output<typeof kinds>;
export function externalApprovalKind(value: unknown): ExternalApprovalKind {
  const result = kinds(value);
  if (
    result.type === 'external_approval_requested' &&
    Date.parse(result.deadline_at) < Date.parse(result.waiting_since)
  )
    throw new ProtocolError('External approval deadline precedes its recorded start');
  return result;
}
