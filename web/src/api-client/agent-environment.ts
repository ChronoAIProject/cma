import {
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from './schema.js';

export const environmentFeature = object({
  state: enumeration(
    'available',
    'unsupported',
    'not_enabled',
    'forbidden',
    'temporarily_unavailable',
  ),
  reason: text,
  scope: text,
  writable: boolean,
});

export const agentEnvironment = object({
  agent_id: text,
  policy_revision: integer,
  runtime_incarnation: nullable(text),
  revalidate_at: nullable(text),
  access_revalidate_at: optional(nullable(text)),
  files: environmentFeature,
  operation_output: environmentFeature,
  browser: environmentFeature,
  replay: environmentFeature,
  control: environmentFeature,
});

export type AgentEnvironment = Output<typeof agentEnvironment>;
export type EnvironmentFeature = Output<typeof environmentFeature>;
