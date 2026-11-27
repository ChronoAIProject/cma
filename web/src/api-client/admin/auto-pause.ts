import {
  array,
  boolean,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../schema.js';

const autoPauseBatch = {
  failed: optional(integer),
  next: optional(nullable(text)),
  enabled: optional(boolean),
  policy_revision: optional(integer),
};
export const autoPausePreview = object({
  candidates: integer,
  refusals: optional(array(object({ sandbox_id: text, reason: text }))),
  ...autoPauseBatch,
});
export type AutoPausePreview = Output<typeof autoPausePreview>;
export const pauseIdleNowRequest = object({
  confirmed: boolean,
  cursor: optional(text),
  limit: optional(integer),
});
export const pauseIdleNow = object({
  scheduled: integer,
  already_satisfied: integer,
  skipped: integer,
  ...autoPauseBatch,
});
export type PauseIdleNow = Output<typeof pauseIdleNow>;
