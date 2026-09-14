import type { ProfileDraft, WorkspaceLocation, WorkspaceProfileContent } from './client';
import { byteLength, validateMetadata, type FieldErrors } from './validation';

export function directoryNameError(name: string): string | undefined {
  return /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$/.test(name)
    ? undefined
    : 'Use up to 64 letters, numbers, dots, underscores or hyphens, starting with a letter or number.';
}

export function repositoryName(url: string): string {
  try {
    return (
      new URL(url).pathname
        .split('/')
        .filter(Boolean)
        .at(-1)
        ?.replace(/\.git$/, '') ?? ''
    );
  } catch {
    return '';
  }
}

export function repositoryKey(location: WorkspaceLocation): string {
  return location.type === 'directory'
    ? ''
    : JSON.stringify([location.url, location.reference ?? '']);
}

export function commandErrors(
  commands: readonly string[],
  maximum: number,
  field: string,
): FieldErrors {
  const errors: Record<string, string> = {};
  if (commands.length > maximum) errors[field] = `Use at most ${String(maximum)} commands.`;
  commands.forEach((command, index) => {
    if (command === '' || byteLength(command) > 4096 || command.includes('\0'))
      errors[`${field}.${String(index)}`] =
        'Enter a command of up to 4,096 bytes without null characters.';
  });
  return errors;
}

export function validateWorkspaceProfile(
  draft: ProfileDraft<WorkspaceProfileContent>,
): FieldErrors {
  const { location, agentProfiles, setupCommands } = draft.content;
  const errors: Record<string, string> = {
    ...validateMetadata(draft),
    ...commandErrors(setupCommands, 16, 'setupCommands'),
  };
  const name =
    draft.content.name || (location.type === 'git_repository' ? repositoryName(location.url) : '');
  const nameError = directoryNameError(name);
  if (nameError !== undefined) errors.directoryName = nameError;
  if (location.type === 'directory') {
    const invalid = directoryNameError(location.name);
    if (invalid !== undefined) errors.locationName = invalid;
  } else {
    try {
      const parsed = new URL(location.url);
      if (
        !location.url.startsWith('https://github.com/') ||
        parsed.origin !== 'https://github.com' ||
        parsed.username !== '' ||
        parsed.password !== '' ||
        parsed.search !== '' ||
        parsed.hash !== '' ||
        parsed.pathname.split('/').filter(Boolean).length !== 2
      )
        errors.repositoryUrl = 'Enter the HTTPS URL of a GitHub repository.';
    } catch {
      errors.repositoryUrl = 'Enter the HTTPS URL of a GitHub repository.';
    }
    if (
      location.reference !== undefined &&
      (location.reference === '' ||
        byteLength(location.reference) > 255 ||
        location.reference.startsWith('-') ||
        /\s/.test(location.reference) ||
        [...location.reference].some(
          (character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127,
        ))
    )
      errors.repositoryReference = 'Enter a branch or tag without spaces, up to 255 bytes.';
  }
  if (agentProfiles.length > 8) errors.agentProfiles = 'Choose at most 8 agent profiles.';
  else if (
    agentProfiles.some((entry) => !Number.isInteger(entry.revision) || entry.revision < 1) ||
    new Set(agentProfiles.map((entry) => `${entry.id}:${String(entry.revision)}`)).size !==
      agentProfiles.length
  )
    errors.agentProfiles = 'Choose each published agent revision only once.';
  return errors;
}
