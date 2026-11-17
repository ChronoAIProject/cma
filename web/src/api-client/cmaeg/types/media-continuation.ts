import {
  boolean,
  integer,
  nullable,
  object,
  optional,
  text,
  enumeration,
  type Output,
} from '../../schema.js';
import { BindingMediaArtifactSchema } from '../../provider-binding-media.js';
export const MediaContinuationKindSchema = enumeration('original', 'transcript', 'text_excerpt');
export type MediaContinuationKind = Output<typeof MediaContinuationKindSchema>;
export const MediaArtifactClaimSchema = BindingMediaArtifactSchema;
export type MediaArtifactClaim = Output<typeof MediaArtifactClaimSchema>;
/** Exact capability comparisons; never selects another target or filesystem path. */
export const EventAttestationRequestSchema = object({
  event_ref: text,
  provider_binding_id: text,
  channel_id: text,
  conversation_id: text,
  event_id: text,
  activity_sha256: text,
  binding_generation: optional(nullable(integer)),
  event_expires_at: integer,
  artifact: optional(nullable(MediaArtifactClaimSchema)),
});
export type EventAttestationRequest = Output<typeof EventAttestationRequestSchema>;
export const EventAttestationResponseSchema = object({ current: boolean });
export type EventAttestationResponse = Output<typeof EventAttestationResponseSchema>;
