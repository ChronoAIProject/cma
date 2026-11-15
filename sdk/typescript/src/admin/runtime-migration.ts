import { ProtocolError } from '../errors.js';
import {
  boolean,
  enumeration,
  object,
  optional,
  positiveInteger,
  record,
  tagged,
  text,
  type Output,
} from '../schema.js';

const originFields = {
  sandbox_id: text,
  owner: text,
  profile_id: text,
  profile_revision: positiveInteger,
  profile_digest: text,
  profile_image: text,
  // Retained provider lineage is opaque to the SDK; preserve it exactly.
  home: record,
};
const origin = object(originFields);
const migrationFields = {
  request_key: text,
  expected_revision: positiveInteger,
  expected_image: text,
  target_image: text,
  origin,
};
const migration = object(migrationFields);
const requestFields = {
  migration,
  authority_id: text,
  expected_current_revision: positiveInteger,
  restart_failed: optional(boolean),
};
const request = object(requestFields);
/** Exact guards only. Approval and activity evidence are resolved by the server. */
export type CustomRuntimeMigrationRequest = Output<typeof request>;

function closed(value: unknown, fields: Readonly<Record<string, unknown>>) {
  const source = record(value);
  if (Object.keys(source).some((key) => !Object.hasOwn(fields, key)))
    throw new ProtocolError('unknown runtime migration field');
  return source;
}
export function customRuntimeMigrationRequest(value: unknown): CustomRuntimeMigrationRequest {
  const source = closed(value, requestFields);
  const requested = closed(source.migration, migrationFields);
  closed(requested.origin, originFields);
  return request(source);
}

/** Unknown and opaque proof references never grant migration permission. */
export const runtimeActivityStatus = tagged(
  {
    unknown: object({
      status: enumeration('unknown'),
      reason: enumeration(
        'unqualified_source',
        'unsupported_context',
        'missing_watermark',
        'malformed_authority',
        'incomplete_coverage',
        'cardinality_exceeded',
        'source_changed',
        'unavailable',
      ),
    }),
    active: object({ status: enumeration('active') }),
    observing: object({ status: enumeration('observing') }),
    quiet: object({ status: enumeration('quiet'), proof_reference: text }),
  },
  'status',
);
export type RuntimeActivityStatus = Output<typeof runtimeActivityStatus>;
