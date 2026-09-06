import type { FieldErrors } from './validation';

export interface ProfileFailure {
  readonly message: string;
  readonly fields: FieldErrors;
  readonly conflict: boolean;
  readonly uncertain: boolean;
}

function record(value: unknown): Record<string, unknown> {
  return typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {};
}

const FIELD_NAMES: Readonly<Record<string, string>> = {
  name: 'name',
  description: 'description',
  model: 'model',
  thinking_effort: 'thinkingEffort',
  permission_level: 'permissionLevel',
  skills: 'skills',
  system_prompt: 'systemPrompt',
};

const REASONS: Readonly<Record<string, string>> = {
  invalid_name: 'Enter a nonempty profile name no longer than 80 bytes.',
  description_too_long: 'Shorten the description to fit 2,000 bytes of text.',
  invalid_model: 'Enter a model or leave it empty for the deployment default.',
  too_many_skills: 'Choose at most 32 skills.',
  invalid_skill: 'Choose a skill with a valid identifier.',
  system_prompt_too_long: 'Shorten the system prompt to fit 32 KB of text.',
  profile_unpublished: 'Publish a revision before making this profile public.',
  private_reference: 'Publish the referenced profiles publicly before sharing this profile.',
  secret_like_environment_value:
    'Remove sensitive environment values before making this profile public.',
};

export function profileFailure(error: unknown): ProfileFailure {
  const problem = record(error);
  const code = typeof problem.code === 'string' ? problem.code : '';
  const reason = typeof problem.reason === 'string' ? problem.reason : '';
  const field = typeof problem.field === 'string' ? FIELD_NAMES[problem.field] : undefined;
  const status = typeof problem.status === 'number' ? problem.status : undefined;
  const conflict = code === 'profile_version_conflict';
  const known: Readonly<Record<string, string>> = {
    profile_version_conflict: 'The saved profile changed. Your draft is still here.',
    profile_not_found: 'This profile is no longer available.',
    forbidden: 'You do not have permission to manage this profile.',
    profile_reference_invalid:
      'A referenced profile is no longer available. Choose another profile.',
    profile_public_refused:
      'This profile cannot be made public. Check its published revision and references.',
    profile_invalid: 'Review the profile fields before trying again.',
    marketplace_disabled: 'The marketplace is currently unavailable.',
  };
  const message =
    REASONS[reason] ??
    known[code] ??
    'The operation could not be confirmed. Retry to check its outcome.';
  return {
    message,
    conflict,
    uncertain: !conflict && (status === undefined || status === 408 || status >= 500),
    fields:
      field === undefined
        ? {}
        : { [field]: REASONS[reason] ?? 'Check this value before trying again.' },
  };
}
