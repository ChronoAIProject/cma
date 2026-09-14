import type { AgentProfileContent, ProfileDraft } from './client';

export type FieldErrors = Readonly<Record<string, string>>;

export function byteLength(value: string): number {
  return new TextEncoder().encode(value).length;
}

export function isSkillVersion(value: string | undefined): value is string {
  return value !== undefined && value.length <= 20 && /^(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(value);
}

export function validateMetadata(
  draft: Pick<ProfileDraft<unknown>, 'name' | 'description'>,
): FieldErrors {
  const errors: Record<string, string> = {};
  if (draft.name.trim() === '') errors.name = 'Enter a profile name.';
  else if (byteLength(draft.name) > 80) errors.name = 'Shorten the name to fit 80 bytes of text.';
  if (byteLength(draft.description) > 2000)
    errors.description = 'Shorten the description to fit 2,000 bytes of text.';
  return errors;
}

export function validateAgentProfile(draft: ProfileDraft<AgentProfileContent>): FieldErrors {
  const errors: Record<string, string> = { ...validateMetadata(draft) };
  if (draft.content.model !== undefined && draft.content.model.trim() === '')
    errors.model = 'Enter a model or leave it empty for the deployment default.';
  if (!['minimal', 'low', 'medium', 'high', 'x_high'].includes(draft.content.thinkingEffort))
    errors.thinkingEffort = 'Choose a thinking effort.';
  if (!['guarded', 'autonomous'].includes(draft.content.permissionLevel))
    errors.permissionLevel = 'Choose how freely the agent can act.';
  if (byteLength(draft.content.systemPrompt) > 32768)
    errors.systemPrompt = 'Shorten the system prompt to fit 32 KB of text.';
  if (draft.content.skills.length > 32) errors.skills = 'Choose at most 32 skills.';
  else if (draft.content.skills.some((skill) => skill.id.trim() === ''))
    errors.skills = 'Choose a skill with a valid identifier.';
  else if (draft.content.skills.some((skill) => !isSkillVersion(skill.version)))
    errors.skills = 'Choose a published major.minor version for each skill.';
  return errors;
}
