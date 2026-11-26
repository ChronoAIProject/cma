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
import { ChannelActivitySchema } from './channel-activity.js';

export const CoverageGapSchema = enumeration(
  'legacy_partial',
  'retention_expired',
  'unreadable_record',
  'capture_gap',
);
export type CoverageGap = Output<typeof CoverageGapSchema>;

export const HistoryRangeSchema = object({ from: text, until: text });
export type HistoryRange = Output<typeof HistoryRangeSchema>;

/** Completeness of receipt enumeration, with current rather than archived summaries. */
export const HistoryCoverageSchema = object({
  complete_event_enumeration: boolean,
  gaps: array(CoverageGapSchema),
  retained_from: text,
  all_writers_capture_from: optional(nullable(text)),
  all_writers_capture_until: optional(nullable(text)),
  current_summaries: boolean,
});
export type HistoryCoverage = Output<typeof HistoryCoverageSchema>;

export const HistoryItemSchema = object({
  /** Opaque details-route identity; activity.event_id is the source identity. */
  history_event_id: text,
  activity: ChannelActivitySchema,
  detail_cursor: optional(nullable(text)),
  detail_incomplete: boolean,
  detail_truncated: boolean,
  legacy_partial: boolean,
});
export type HistoryItem = Output<typeof HistoryItemSchema>;

export const HistoryPageSchema = object({
  items: array(HistoryItemSchema),
  next_cursor: optional(nullable(text)),
  has_more: boolean,
  range: HistoryRangeSchema,
  coverage: HistoryCoverageSchema,
});
export type HistoryPage = Output<typeof HistoryPageSchema>;

export const HistoryDetailPartSchema = object({ index: integer, sha256: text, data: text });
export type HistoryDetailPart = Output<typeof HistoryDetailPartSchema>;

/** Concatenate decoded parts in index order before deserializing ChannelActivity. */
export const HistoryDetailsSchema = object({
  generation: text,
  encoding: text,
  parts: array(HistoryDetailPartSchema),
  next_cursor: optional(nullable(text)),
  detail_incomplete: boolean,
  detail_truncated: boolean,
});
export type HistoryDetails = Output<typeof HistoryDetailsSchema>;
