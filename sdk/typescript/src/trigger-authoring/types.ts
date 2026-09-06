import { profileReference } from '../profiles/types.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  object,
  optional,
  positiveInteger,
  text,
  type Output,
} from '../schema.js';

export const triggerButtonLabel = enumeration(
  'open_in_cma',
  'chat_with_this',
  'run_with_cma',
  'try_it',
);
export type TriggerButtonLabel = Output<typeof triggerButtonLabel>;
export const triggerTheme = enumeration('auto', 'light', 'dark');
export type TriggerTheme = Output<typeof triggerTheme>;
export const triggerContent = object({
  presentation: object({
    title: text,
    description: text,
    button_label: triggerButtonLabel,
    theme: triggerTheme,
    signup_hint: optional(text),
  }),
  target: object({
    sandbox_profile: profileReference,
    workspace_profile: profileReference,
    agent_profile: profileReference,
  }),
  reuse: object({
    sandbox: enumeration('reuse_existing', 'always_new'),
    workspace: enumeration('reuse_existing', 'always_new'),
    agent: enumeration('resume_existing', 'always_new', 'ask'),
  }),
  first_message: text,
  consent: object({ always_review: boolean }),
});
export type TriggerContent = Output<typeof triggerContent>;
export const triggerAvailability = enumeration('draft', 'published', 'retired');
export const triggerPlacement = object({
  id: text,
  source_url: text,
  label: text,
  retired_at: optional(text),
});
export type TriggerPlacement = Output<typeof triggerPlacement>;
export const triggerTotals = object({ launches_total: integer, agents_ready_total: integer });
export const triggerDocument = object({
  id: text,
  draft: triggerContent,
  draft_version: positiveInteger,
  public_id: optional(text),
  availability: triggerAvailability,
  revision_count: integer,
  placements: array(triggerPlacement),
  stats: triggerTotals,
  created_at: text,
  updated_at: text,
  retired_at: optional(text),
});
export type TriggerDocument = Output<typeof triggerDocument>;
const revisionFields = {
  revision: positiveInteger,
  content: triggerContent,
  digest: text,
  published_at: text,
};
export const triggerRevision = object(revisionFields);
export type TriggerRevision = Output<typeof triggerRevision>;
export const publishedTriggerRevision = object({
  trigger_id: text,
  public_id: text,
  availability: triggerAvailability,
  ...revisionFields,
});
export type PublishedTriggerRevision = Output<typeof publishedTriggerRevision>;
export const triggerStatistics = object({
  totals: triggerTotals,
  placements: array(
    object({ placement_id: optional(text), launches_total: integer, agents_ready_total: integer }),
  ),
});
export type TriggerStatistics = Output<typeof triggerStatistics>;
export const triggerEmbed = object({
  public_url: text,
  badge_url: text,
  label_text: text,
  javascript_sri: text,
  content_security_policy: object({ img_src: text, script_src: text }),
  snippets: object({ markdown: text, html: text, javascript: text, react: text, url: text }),
});
export type TriggerEmbed = Output<typeof triggerEmbed>;
export const createTriggerBody = object({ content: triggerContent });
export const saveTriggerBody = object({ draft_version: positiveInteger, content: triggerContent });
export const publishTriggerBody = object({ draft_version: positiveInteger });
export const createPlacementBody = object({ source_url: text, label: text });
