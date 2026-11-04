import type {
  ProfileKind,
  TriggerContent,
  TriggerDocument,
  TriggerInlineTarget,
} from '@chronoai/cma-sdk';
import type { StatusTone } from '../vocabulary/status';
import type { PublisherProfile } from './client';

export type SelectedProfiles = Partial<Record<ProfileKind, PublisherProfile>>;
export const STEP_NAMES = [
  'What it opens',
  'When someone clicks again',
  'What it says',
  'Where it lives',
  'Publish',
] as const;
/**
 * A Trigger's status as a word and tone — `docs/Web_Style.md` §12.1. Only a
 * published Trigger is live for visitors; a draft and a retired one are not.
 */
export const AVAILABILITY = {
  draft: { label: 'Draft', tone: 'muted' },
  published: { label: 'Published', tone: 'neutral' },
  retired: { label: 'Retired', tone: 'muted' },
} as const satisfies Record<
  TriggerDocument['availability'],
  { readonly label: string; readonly tone: StatusTone }
>;
export const LABELS = {
  open_in_cma: 'Open in CMA',
  chat_with_this: 'Chat with this',
  run_with_cma: 'Run with CMA',
  try_it: 'Try it',
} as const;
export const blankContent = (): TriggerContent => ({
  presentation: { title: '', description: '', button_label: 'open_in_cma', theme: 'auto' },
  target: {
    sandbox_profile: { id: '', revision: 1 },
    workspace_profile: { id: '', revision: 1 },
    agent_profile: { id: '', revision: 1 },
    inline: {
      agent: {
        reasoning_effort: 'medium',
        permission_mode: 'guarded',
        skills: [],
        system_prompt: '',
        services: [],
      },
    },
  },
  reuse: { sandbox: 'reuse_existing', workspace: 'reuse_existing', agent: 'ask' },
  first_message: '',
  consent: { always_review: true },
});
const bytes = (value: string) => new TextEncoder().encode(value).length;

export function validateStep(
  step: number,
  content: TriggerContent,
  profiles: SelectedProfiles,
): Record<string, string> {
  const errors: Record<string, string> = {};
  if (step === 0 && content.target.inline !== undefined) {
    const inline = content.target.inline;
    if (inline.image?.id.trim() === '')
      errors['target.inline.image'] = 'Choose an image or use the system default.';
    else if (inline.image !== undefined && inline.image.reference === undefined)
      errors['target.inline.image'] = 'The selected image has no immutable successful version.';
    if (inline.agent.model !== undefined && inline.agent.model.trim() === '')
      errors['target.inline.agent.model'] = 'Enter a model or leave it at the deployment default.';
    if (inline.agent.skills.length > 32)
      errors['target.inline.agent.skills'] = 'Choose at most 32 skills.';
    if (inline.agent.services.length > 64)
      errors['target.inline.agent.services'] = 'Choose at most 64 services.';
    const repository = inline.workspace?.repository;
    if (repository !== undefined) {
      try {
        const url = new URL(repository.url);
        if (
          url.protocol !== 'https:' ||
          url.hostname !== 'github.com' ||
          url.username !== '' ||
          url.password !== '' ||
          url.port !== '' ||
          url.search !== '' ||
          url.hash !== '' ||
          url.pathname.split('/').filter(Boolean).length !== 2
        )
          errors['target.inline.workspace.repository'] = 'Enter an HTTPS GitHub repository URL.';
      } catch {
        errors['target.inline.workspace.repository'] = 'Enter an HTTPS GitHub repository URL.';
      }
    }
  } else if (step === 0)
    for (const kind of ['sandbox', 'workspace', 'agent'] as const) {
      const reference = content.target[`${kind}_profile`];
      const selected = profiles[kind];
      if (
        reference.id === '' ||
        selected === undefined ||
        selected.id !== reference.id ||
        selected.revision !== reference.revision
      )
        errors[`target.${kind}_profile`] = `Choose an available ${kind} profile revision.`;
      else if (!selected.public)
        errors[`target.${kind}_profile`] = 'Make this profile public before publishing a Trigger.';
    }
  if (step === 2) {
    if (content.presentation.title.trim() === '' || bytes(content.presentation.title) > 120)
      errors['presentation.title'] = 'Enter a title of 1 to 120 bytes.';
    if (bytes(content.presentation.description) > 2_000)
      errors['presentation.description'] = 'Keep the description within 2,000 bytes.';
    if (bytes(content.first_message) > 32_768)
      errors.first_message = 'Keep the first message within 32,768 bytes.';
    if (bytes(content.presentation.signup_hint ?? '') > 200)
      errors['presentation.signup_hint'] = 'Keep the sign-up hint within 200 bytes.';
    const messages = content.quick_messages ?? [];
    if (messages.length > 8) errors.quick_messages = 'Keep at most eight quick messages.';
    const names = new Set<string>();
    for (const message of messages) {
      const name = message.name.trim().toLowerCase();
      if (name === '' || bytes(message.name) > 120 || /\p{Cc}/u.test(message.name))
        errors.quick_messages =
          'Each quick message needs a name within 120 bytes, without control characters.';
      else if (names.has(name)) errors.quick_messages = 'Give each quick message a different name.';
      else if (message.description.trim() === '' || bytes(message.description) > 2_000)
        errors.quick_messages = 'Each quick message needs a description within 2,000 bytes.';
      else if (message.text.trim() === '' || bytes(message.text) > 32_768)
        errors.quick_messages = 'Each quick message needs message text within 32,768 bytes.';
      names.add(name);
    }
  }
  return errors;
}

export function reuseExplanation(reuse: TriggerContent['reuse']): string {
  if (reuse.sandbox === 'always_new')
    return 'Every click creates a new Sandbox, Workspace, and Agent for the visitor.';
  if (reuse.workspace === 'always_new')
    return 'The visitor keeps their matching Sandbox. Every click creates a new Workspace and Agent inside it.';
  if (reuse.agent === 'always_new')
    return 'The visitor keeps their matching Sandbox and Workspace. Every click starts a new Agent there.';
  if (reuse.agent === 'ask')
    return 'The visitor keeps their matching Sandbox and Workspace and chooses whether to continue their previous Agent or start a new one.';
  return 'The visitor continues their previous Agent in their matching Workspace and Sandbox. If no match exists, CMA creates it.';
}

export function profileSummary(profile: PublisherProfile): string {
  if (profile.kind === 'sandbox')
    return `${String(profile.content.cpu_millis)} millicores, ${String(profile.content.memory_mib)} MiB; ${profile.content.nyxid_services.join(', ') || 'no connected services'}`;
  if (profile.kind === 'workspace')
    return profile.content.location.type !== 'directory'
      ? profile.content.location.url
      : profile.content.location.name;
  return `${profile.content.model ?? 'Deployment model'}, ${profile.content.thinking_effort} effort, ${profile.content.permission_level}`;
}

export function resultSentence(profiles: SelectedProfiles): string {
  const { sandbox, workspace, agent } = profiles;
  if (sandbox?.kind !== 'sandbox' || workspace === undefined || agent === undefined) return '';
  return `Visitors get ${agent.name} in a ${workspace.name} workspace inside a ${sandbox.name} sandbox${sandbox.content.nyxid_services.length === 0 ? '.' : ` that reaches ${sandbox.content.nyxid_services.join(', ')}.`}`;
}

export function inlineResultSentence(target: TriggerInlineTarget): string {
  const image = target.image?.id ?? 'the system default image';
  const workspace = target.workspace?.repository?.url ?? 'an empty workspace';
  const services = target.agent.services;
  return `Visitors get an inline Agent from ${image} in ${workspace}${services.length === 0 ? '.' : ` with ${services.join(', ')} access.`}`;
}

export function placementError(label: string, source: string): string | undefined {
  if (bytes(label) > 80) return 'Keep the placement label within 80 bytes.';
  try {
    const url = new URL(source);
    if (
      url.protocol !== 'https:' ||
      url.username !== '' ||
      url.password !== '' ||
      bytes(source) > 512
    )
      return 'Enter an HTTPS source URL within 512 bytes and without credentials.';
  } catch {
    return 'Enter a public HTTPS source URL.';
  }
  return undefined;
}
