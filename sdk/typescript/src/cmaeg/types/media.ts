import { AttachmentKindSchema, MediaVariantSchema } from './primitives.js';
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
} from '../../schema.js';

export const AttachmentViewKindSchema = enumeration('auto', 'image', 'text', 'frames', 'download');
export type AttachmentViewKind = Output<typeof AttachmentViewKindSchema>;

export const AttachmentStatusSchema = enumeration('ready', 'pending', 'unavailable');
export type AttachmentStatus = Output<typeof AttachmentStatusSchema>;

export const AttachmentUnavailableReasonSchema = enumeration(
  'not_retrievable',
  'not_found',
  'too_large',
  'too_large_pixels',
  'unsupported_format',
  'decode_failed',
  'timed_out',
  'source_unavailable',
  'transcriber_disabled',
  'transcriber_failed',
  'too_long',
  'rate_limited',
  'transform_failed',
  'download_not_permitted',
  'link_unavailable',
  'images_truncated',
  'response_budget',
);
export type AttachmentUnavailableReason = Output<typeof AttachmentUnavailableReasonSchema>;

export const AttachmentTextSourceSchema = enumeration('transcript', 'document');
export type AttachmentTextSource = Output<typeof AttachmentTextSourceSchema>;

// Decided per image: one set of frames can mix keyframes with a thumbnail.
export const AttachmentImageOriginSchema = enumeration(
  'normalized',
  'platform_thumbnail',
  'keyframe',
);
export type AttachmentImageOrigin = Output<typeof AttachmentImageOriginSchema>;

export const AttachmentTextSchema = object({
  content: text,
  page: integer,
  pages: integer,
  source: AttachmentTextSourceSchema,
  truncated: boolean,
});
export type AttachmentText = Output<typeof AttachmentTextSchema>;

// Metadata only: image bytes travel as MCP image content, never in a view.
export const AttachmentImageSchema = object({
  at_ms: optional(nullable(integer)),
  bytes: integer,
  height: integer,
  mime_type: text,
  origin: AttachmentImageOriginSchema,
  width: integer,
});
export type AttachmentImage = Output<typeof AttachmentImageSchema>;

// The structured header credential for clients that cannot execute shell text.
export const MediaAuthorizationSchema = object({
  scheme: text,
  token: text,
});
export type MediaAuthorization = Output<typeof MediaAuthorizationSchema>;

// A ready download: `authorization` and `curl` carry the same single-use link.
export const MediaDownloadSchema = object({
  authorization: MediaAuthorizationSchema,
  curl: text,
  expires_at: integer,
  method: text,
  url: text,
});
export type MediaDownload = Output<typeof MediaDownloadSchema>;

export const AttachmentViewSchema = object({
  detected_type: optional(nullable(text)),
  download: optional(nullable(MediaDownloadSchema)),
  duration_seconds: optional(nullable(integer)),
  filename: optional(nullable(text)),
  height: optional(nullable(integer)),
  images: optional(array(AttachmentImageSchema)),
  index: integer,
  kind: AttachmentKindSchema,
  mime_type: optional(nullable(text)),
  notice: optional(nullable(text)),
  reason: optional(nullable(AttachmentUnavailableReasonSchema)),
  retry_after_ms: optional(nullable(integer)),
  size_bytes: optional(nullable(integer)),
  status: AttachmentStatusSchema,
  text: optional(nullable(AttachmentTextSchema)),
  variant: optional(nullable(MediaVariantSchema)),
  width: optional(nullable(integer)),
});
export type AttachmentView = Output<typeof AttachmentViewSchema>;

export const VisualSupportSchema = enumeration('none', 'platform_thumbnail', 'keyframes');
export type VisualSupport = Output<typeof VisualSupportSchema>;

export const InboundMediaCapabilitiesSchema = object({
  downloads: boolean,
  read_tool: boolean,
  transcripts: boolean,
  visuals: VisualSupportSchema,
});
export type InboundMediaCapabilities = Output<typeof InboundMediaCapabilitiesSchema>;
