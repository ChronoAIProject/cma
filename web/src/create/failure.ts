import { profileFailure, type ProfileFailure } from '../profiles/failure';

const messages: Readonly<Record<string, string>> = {
  forbidden: 'You do not have permission to create this resource.',
  invalid_sandbox_name: 'Use a nonempty sandbox name no longer than 64 bytes.',
  sandbox_name_conflict: 'A sandbox already has this name. Choose another name.',
  workspace_name_conflict: 'This workspace name is already in use. Choose another profile.',
  sandbox_not_ready: 'The sandbox is not ready. Wait for preparation or choose another sandbox.',
  workspace_not_ready:
    'The workspace is not ready. Wait for preparation or choose another workspace.',
  sandbox_not_found: 'The selected sandbox is no longer available.',
  workspace_not_found: 'The selected workspace is no longer available.',
  sandbox_profile_not_found:
    'The selected sandbox revision is no longer available. Choose another profile.',
  workspace_profile_not_found:
    'The selected workspace revision is no longer available. Choose another profile.',
  agent_profile_not_found:
    'The selected agent revision is no longer available. Choose another profile.',
  sandbox_limit_reached:
    'Your sandbox limit has been reached. Delete an unused sandbox before creating another.',
  workspace_limit_reached:
    'This sandbox has reached its workspace limit. Choose another sandbox or remove an unused workspace.',
  agent_limit_reached:
    'This workspace has reached its agent limit. Choose another workspace or stop an unused agent.',
  trust_confirmation_required:
    'Choose the profile again and review its commands before confirming trust.',
};

export function creationFailure(error: unknown): ProfileFailure {
  const failure = profileFailure(error);
  const code =
    typeof error === 'object' && error !== null && 'code' in error && typeof error.code === 'string'
      ? error.code
      : '';
  return {
    ...failure,
    message:
      messages[code] ??
      (failure.uncertain
        ? failure.message
        : 'Creation was refused. Review the selected profile and location before trying again.'),
  };
}
