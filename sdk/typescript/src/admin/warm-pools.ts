import { ValidationError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../schema.js';

export const warmPoolView = object({
  id: text,
  revision: integer,
  image: text,
  config_sha256: optional(text),
  cpu_millis: integer,
  memory_mib: integer,
  target_replicas: integer,
  ready_replicas: nullable(integer),
  observed: boolean,
  status: enumeration('pending', 'ready', 'disabled', 'unavailable'),
  resource_name: nullable(text),
  error: nullable(text),
});
export type WarmPoolView = Output<typeof warmPoolView>;
export const warmPoolList = object({
  items: array(warmPoolView),
  default_image: text,
  default_cpu_millis: integer,
  default_memory_mib: integer,
  default_config_sha256: optional(nullable(text)),
  available: boolean,
});
export type WarmPoolList = Output<typeof warmPoolList>;
export interface ConfigureWarmPoolBody {
  readonly cpu_millis: number;
  readonly memory_mib: number;
  readonly target_replicas: number;
}
export interface ResizeWarmPoolBody {
  readonly expected_revision: number;
  readonly target_replicas: number;
}
export function validateWarmPoolTarget(body: ConfigureWarmPoolBody | ResizeWarmPoolBody) {
  if (
    !Number.isInteger(body.target_replicas) ||
    body.target_replicas < 0 ||
    body.target_replicas > 1000
  )
    throw new ValidationError('target_replicas must be an integer between 0 and 1000');
  if ('cpu_millis' in body) {
    for (const value of [body.cpu_millis, body.memory_mib])
      if (!Number.isInteger(value) || value < 1 || value > 1_048_576)
        throw new ValidationError(
          'pool CPU and memory must be positive integers within deployment limits',
        );
  } else if (!Number.isSafeInteger(body.expected_revision) || body.expected_revision < 1) {
    throw new ValidationError('expected_revision must be a positive integer');
  }
}
