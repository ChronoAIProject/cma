import type { ResolvedConfig } from '../config.js';
import type { OperationOptions } from '../http.js';
import { sandbox } from '../instances/types.js';
import { identity, input, read, segment } from '../resources.js';
import {
  boolean,
  enumeration,
  integer,
  object,
  optional,
  record,
  tagged,
  text,
  type Output,
} from '../schema.js';

const request = object({
  migration: object({
    request_key: text,
    expected_revision: integer,
    expected_image: text,
    target_image: text,
    origin: object({
      sandbox_id: text,
      owner: text,
      profile_id: text,
      profile_revision: integer,
      profile_digest: text,
      profile_image: text,
      home: object({ request: record, receipt: record }),
    }),
  }),
  authority_id: text,
  expected_current_revision: integer,
  restart_failed: optional(boolean),
});

const activity = tagged(
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

export type CustomRuntimeMigrationRequest = Output<typeof request>;
export type RuntimeActivityStatus = Output<typeof activity>;

export function requestRuntimeMigration(
  config: ResolvedConfig,
  id: string,
  body: CustomRuntimeMigrationRequest,
  options: OperationOptions,
) {
  return read(
    config,
    `/api/v1/admin/sandboxes/${segment(id)}/runtime-migration`,
    identity(sandbox, id),
    { ...options, method: 'POST', body: input(request, body), expectedStatus: 202 },
  );
}

export function readRuntimeActivity(config: ResolvedConfig, id: string, options: OperationOptions) {
  return read(
    config,
    `/api/v1/admin/sandboxes/${segment(id)}/runtime-update/activity`,
    activity,
    options,
  );
}
