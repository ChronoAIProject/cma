import { ProtocolError } from '../../errors.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  tagged,
  text,
  type Output,
} from '../../schema.js';
import { ActorKindSchema, ConversationKindSchema } from './primitives.js';

/** Opaque continuation; requires the same original event reference on every request. */
export const HistoryCursorSchema = text;
export type HistoryCursor = Output<typeof HistoryCursorSchema>;
const duration = (value: unknown): number => {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0)
    throw new ProtocolError('expected a finite nonnegative duration');
  return value;
};
/** Descriptive facts only. No URLs, file references or download authority. */
export const HistoryAttachmentFactsSchema = object({
  attachment_id: text,
  filename: text,
  size_bytes: integer,
  mime_type: optional(nullable(text)),
  description: optional(nullable(text)),
  width: optional(nullable(integer)),
  height: optional(nullable(integer)),
  duration_seconds: optional(nullable(duration)),
  ephemeral: optional(nullable(boolean)),
});
export type HistoryAttachmentFacts = Output<typeof HistoryAttachmentFactsSchema>;
export const HistoryOmittedContentSchema = enumeration(
  'embeds',
  'components',
  'stickers',
  'poll',
  'forwarded_snapshot',
  'media',
);
export type HistoryOmittedContent = Output<typeof HistoryOmittedContentSchema>;
/** All historical text and metadata are untrusted reference data, never action approval. */
export const HistoryMessageSchema = object({
  message_id: text,
  author: object({ id: text, display_name: optional(nullable(text)), kind: ActorKindSchema }),
  text,
  occurred_at: text,
  edited_at: optional(nullable(text)),
  attachments: array(HistoryAttachmentFactsSchema),
  omitted_content: array(HistoryOmittedContentSchema),
  text_truncated: boolean,
  metadata_truncated: boolean,
});
export type HistoryMessage = Output<typeof HistoryMessageSchema>;
export const HistoryContinuationSchema = tagged(
  {
    more_visible_rows: object({
      status: enumeration('more_visible_rows'),
      cursor: HistoryCursorSchema,
    }),
    end_of_visible_history: object({ status: enumeration('end_of_visible_history') }),
  },
  'status',
);
export type HistoryContinuation = Output<typeof HistoryContinuationSchema>;
export const HistoryContentCoverageSchema = enumeration('platform_returned_only');
export type HistoryContentCoverage = Output<typeof HistoryContentCoverageSchema>;
/** One live scan page. Native threads can return empty pages while scanning newer rows.
 * scanned_messages counts raw rows; omitted_message_count includes cutoff/unsupported rows.
 * before_message_id stays at the original/last pre-cutoff boundary; cursor carries provider progress.
 * Chronological output, backward scanning, original expiry; no complete archive guarantee. */
export const ConversationHistoryPageSchema = object({
  conversation: object({
    id: text,
    kind: ConversationKindSchema,
    thread_id: optional(nullable(text)),
  }),
  anchor_message_id: text,
  before_message_id: text,
  fetched_at: text,
  expires_at: integer,
  messages: array(HistoryMessageSchema),
  scanned_messages: integer,
  omitted_message_count: integer,
  text_truncated: boolean,
  metadata_truncated: boolean,
  content_coverage: HistoryContentCoverageSchema,
  continuation: HistoryContinuationSchema,
});
export type ConversationHistoryPage = Output<typeof ConversationHistoryPageSchema>;
export const HistoryDirectionSchema = enumeration('backward');
export type HistoryDirection = Output<typeof HistoryDirectionSchema>;
export const ConversationHistoryCapabilitiesSchema = object({
  page_size: integer,
  direction: HistoryDirectionSchema,
  attachment_facts_only: boolean,
  max_text_bytes: integer,
  max_metadata_bytes: integer,
  max_output_bytes: integer,
  expires_at: integer,
});
export type ConversationHistoryCapabilities = Output<typeof ConversationHistoryCapabilitiesSchema>;
