import { permissionLevel, skillReference, thinkingEffort } from '../profiles/types.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  object,
  optional,
  positiveInteger,
  record,
  text,
  type Output,
} from '../schema.js';
import { ProtocolError } from '../errors.js';

export const triggerButtonLabel = enumeration(
  'open_in_cma',
  'chat_with_this',
  'run_with_cma',
  'try_it',
);
export type TriggerButtonLabel = Output<typeof triggerButtonLabel>;
export const triggerTheme = enumeration('auto', 'light', 'dark');
export type TriggerTheme = Output<typeof triggerTheme>;
export const triggerQuickMessage = object({ name: text, description: text, text });
export type TriggerQuickMessage = Output<typeof triggerQuickMessage>;
export const triggerImage = object({
  id: text,
  version: optional(text),
  reference: optional(text),
});
export type TriggerImage = Output<typeof triggerImage>;
const repositoryFields = object({ url: text, ref: optional(text) });
export const triggerRepository = (value: unknown) => {
  if (Object.keys(record(value)).some((key) => !['url', 'ref'].includes(key)))
    throw new ProtocolError('unknown Trigger repository field');
  return repositoryFields(value);
};
export type TriggerRepository = Output<typeof triggerRepository>;
export const triggerInlineWorkspace = object({ repository: optional(triggerRepository) });
export type TriggerInlineWorkspace = Output<typeof triggerInlineWorkspace>;
export const triggerInlineAgent = object({
  model: optional(text),
  reasoning_effort: thinkingEffort,
  permission_mode: permissionLevel,
  skills: array(skillReference),
  system_prompt: text,
  services: array(text),
});
export type TriggerInlineAgent = Output<typeof triggerInlineAgent>;
export const triggerInlineTarget = object({
  image: optional(triggerImage),
  agent: triggerInlineAgent,
  workspace: optional(triggerInlineWorkspace),
});
export type TriggerInlineTarget = Output<typeof triggerInlineTarget>;
const triggerProfileReference = object({ id: text, revision: integer });
const normalizeTriggerProfileReference = (value: unknown) => {
  const reference = triggerProfileReference(value);
  if (reference.revision === 0) {
    if (reference.id !== '') throw new ProtocolError('profile revision must be positive');
    // Inline targets keep legacy profile fields as empty sentinels. Normalize
    // the response so callers can continue using the stable reference shape.
    return { id: '', revision: 1 };
  }
  return reference;
};
const triggerTarget = (value: unknown) => {
  const parsed = object({
    sandbox_profile: optional(triggerProfileReference),
    workspace_profile: optional(triggerProfileReference),
    agent_profile: optional(triggerProfileReference),
    inline: optional(triggerInlineTarget),
  })(value);
  if (parsed.inline === undefined) {
    if (
      parsed.sandbox_profile === undefined ||
      parsed.workspace_profile === undefined ||
      parsed.agent_profile === undefined ||
      parsed.sandbox_profile.revision === 0 ||
      parsed.workspace_profile.revision === 0 ||
      parsed.agent_profile.revision === 0
    )
      throw new ProtocolError('profile targets are required when inline configuration is absent');
  }
  return {
    sandbox_profile: normalizeTriggerProfileReference(
      parsed.sandbox_profile ?? { id: '', revision: 0 },
    ),
    workspace_profile: normalizeTriggerProfileReference(
      parsed.workspace_profile ?? { id: '', revision: 0 },
    ),
    agent_profile: normalizeTriggerProfileReference(
      parsed.agent_profile ?? { id: '', revision: 0 },
    ),
    ...(parsed.inline === undefined ? {} : { inline: parsed.inline }),
  };
};
export const triggerContent = object({
  presentation: object({
    title: text,
    description: text,
    button_label: triggerButtonLabel,
    theme: triggerTheme,
    signup_hint: optional(text),
  }),
  target: triggerTarget,
  reuse: object({
    sandbox: enumeration('reuse_existing', 'always_new'),
    workspace: enumeration('reuse_existing', 'always_new'),
    agent: enumeration('resume_existing', 'always_new', 'ask'),
  }),
  first_message: text,
  quick_messages: optional(array(triggerQuickMessage)),
  consent: object({ always_review: boolean }),
});
export type TriggerContent = Output<typeof triggerContent>;
/** New Trigger inputs can configure an Agent without selecting legacy profiles. */
export type TriggerContentInput = Omit<TriggerContent, 'target'> & {
  readonly target:
    | TriggerContent['target']
    | (Partial<Omit<TriggerContent['target'], 'inline'>> & {
        readonly inline: TriggerInlineTarget;
      });
};
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
