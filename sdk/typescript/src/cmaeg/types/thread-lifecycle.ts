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
import { EventRefSchema } from './primitives.js';

/** Opaque original-event authority. Never log or transfer across consumers. */
export const ThreadRefSchema = text;
export type ThreadRef = Output<typeof ThreadRefSchema>;
export const ThreadListViewSchema = enumeration('active', 'archived');
export type ThreadListView = Output<typeof ThreadListViewSchema>;
export const CreateThreadRequestSchema = object({ name: text, auto_archive_duration: integer });
export type CreateThreadRequest = Output<typeof CreateThreadRequestSchema>;
export const ThreadActionSchema = enumeration('close', 'archive', 'reopen', 'join', 'leave');
export type ThreadAction = Output<typeof ThreadActionSchema>;
export const ThreadTargetSchema = tagged(
  {
    event: object({ type: enumeration('event'), event_ref: EventRefSchema }),
    thread: object({ type: enumeration('thread'), thread_ref: ThreadRefSchema }),
  },
  'type',
);
export type ThreadTarget = Output<typeof ThreadTargetSchema>;
export const ManageThreadRequestSchema = object({
  target: ThreadTargetSchema,
  action: ThreadActionSchema,
});
export type ManageThreadRequest = Output<typeof ManageThreadRequestSchema>;
export const ThreadSummarySchema = object({
  thread_ref: ThreadRefSchema,
  thread_id: text,
  parent_id: text,
  name: text,
  archived: boolean,
  locked: boolean,
  expires_at: integer,
});
export type ThreadSummary = Output<typeof ThreadSummarySchema>;
export const ThreadListScopeSchema = enumeration(
  'original_thread',
  'public_children_of_original_parent',
);
export type ThreadListScope = Output<typeof ThreadListScopeSchema>;
export const ThreadListSchema = object({
  threads: array(ThreadSummarySchema),
  truncated: boolean,
  scope: ThreadListScopeSchema,
});
export type ThreadList = Output<typeof ThreadListSchema>;
export const ThreadOutcomeSchema = tagged(
  {
    confirmed: object({ status: enumeration('confirmed') }),
    unsupported: object({ status: enumeration('unsupported'), reason: text }),
    rejected: object({ status: enumeration('rejected'), reason: text }),
    rate_limited: object({ status: enumeration('rate_limited'), retry_after: integer }),
    unknown: object({ status: enumeration('unknown'), reason: text }),
  },
  'status',
);
export type ThreadOutcome = Output<typeof ThreadOutcomeSchema>;
export const ThreadReceiptSchema = object({
  outcome: ThreadOutcomeSchema,
  thread: optional(nullable(ThreadSummarySchema)),
});
export type ThreadReceipt = Output<typeof ThreadReceiptSchema>;
/** Scope availability; Discord still checks the bot's actual permissions. */
export const ThreadCapabilitiesSchema = object({
  create_from_original_message: boolean,
  list: ThreadListScopeSchema,
  manage_original_thread: boolean,
  actions: array(ThreadActionSchema),
  expires_at: integer,
});
export type ThreadCapabilities = Output<typeof ThreadCapabilitiesSchema>;

/** Exact admitted Telegram forum topic; never grants creation or listing. */
export const TopicActionSchema = enumeration('close', 'reopen');
export type TopicAction = Output<typeof TopicActionSchema>;
export const TopicLifecycleCapabilitiesSchema = object({
  manage_original_topic: boolean,
  actions: array(TopicActionSchema),
  expires_at: integer,
});
export type TopicLifecycleCapabilities = Output<typeof TopicLifecycleCapabilitiesSchema>;
