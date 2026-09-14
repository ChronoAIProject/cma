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
  'availability.warmup_count': 'warmupCount',
  'availability.warmup_enabled': 'warmupEnabled',
  name: 'name',
  description: 'description',
  model: 'model',
  thinking_effort: 'thinkingEffort',
  permission_level: 'permissionLevel',
  skills: 'skills',
  system_prompt: 'systemPrompt',
  'location.url': 'repositoryUrl',
  'location.reference': 'repositoryReference',
  'location.name': 'locationName',
  agent_profiles: 'agentProfiles',
  workspace_profiles: 'workspaceProfiles',
  cpu_millis: 'cpuMillis',
  memory_mib: 'memoryMib',
  max_workspaces: 'maxWorkspaces',
  max_agents_per_workspace: 'maxAgentsPerWorkspace',
  environment_variables: 'environmentVariables',
  'environment_variables.name': 'environmentVariables',
  'environment_variables.value': 'environmentVariables',
  commands: 'commands',
};

const REASONS: Readonly<Record<string, string>> = {
  invalid_warmup_count: 'Choose a whole number from 1 to 5.',
  warmup_requires_public_profile: 'Make this profile public before enabling warm up.',
  warmup_requires_published_profile: 'Publish this profile before enabling warm up.',
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
  invalid_repository: 'Enter the HTTPS URL of a GitHub repository.',
  invalid_repository_reference: 'Check the repository branch or tag.',
  invalid_workspace_name: 'Choose a valid directory name.',
  invalid_command: 'Each command must fit 4,096 bytes; multiline commands need a complete heredoc.',
  too_many_commands: 'Reduce the number of commands in this profile.',
  reserved_environment_variable:
    'Choose an environment variable name that is not reserved by the system.',
  duplicate_environment_variable: 'Give each environment variable a different name.',
  invalid_environment_value: 'Remove null characters from the environment value.',
  cpu_out_of_bounds: 'Choose a CPU size within the deployment limits.',
  memory_out_of_bounds: 'Choose a memory size within the deployment limits.',
};

export function profileFailure(error: unknown): ProfileFailure {
  const problem = record(error);
  const code = typeof problem.code === 'string' ? problem.code : '';
  const reason = typeof problem.reason === 'string' ? problem.reason : '';
  const variable =
    typeof problem.field === 'string'
      ? /^environment_variables\.([A-Z_][A-Z0-9_]*)$/.exec(problem.field)?.[1]
      : undefined;
  const path = typeof problem.path === 'string' ? problem.path.split(/[.[]/, 1)[0] : undefined;
  const field =
    reason === 'invalid_workspace_name'
      ? 'directoryName'
      : variable !== undefined
        ? 'environmentVariables'
        : typeof problem.field === 'string'
          ? FIELD_NAMES[problem.field]
          : path === undefined
            ? undefined
            : FIELD_NAMES[path];
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
    profile_secret_scanner_unavailable:
      'Environment values could not be checked. This profile was not published publicly.',
    idempotency_in_flight: 'This command is still being processed. Retry to check its outcome.',
    idempotency_recovery_required:
      'This command needs recovery. Its original request has been retained.',
  };
  const validation =
    reason === 'secret_like_environment_value' && variable !== undefined
      ? `Remove the sensitive value for ${variable} before making this profile public.`
      : REASONS[reason];
  const message =
    validation ??
    known[code] ??
    'The operation could not be confirmed. Retry to check its outcome.';
  return {
    message,
    conflict,
    uncertain:
      !conflict &&
      (code === 'idempotency_in_flight' ||
        code === 'idempotency_recovery_required' ||
        status === undefined ||
        status === 408 ||
        status >= 500),
    fields:
      field === undefined ? {} : { [field]: validation ?? 'Check this value before trying again.' },
  };
}
