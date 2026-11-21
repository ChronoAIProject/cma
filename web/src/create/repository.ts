/**
 * The optional repository a new Agent prepares in its empty workspace.
 *
 * Only a plain HTTPS github.com owner/repository URL passes. Where the check
 * runs is the dialog's concern (on blur and on submit); what it accepts is not
 * relaxed there.
 */
export function validRepository(value: string): boolean {
  if (value === '') return true;
  try {
    const url = new URL(value);
    return (
      url.protocol === 'https:' &&
      url.hostname === 'github.com' &&
      url.username === '' &&
      url.password === '' &&
      url.port === '' &&
      url.search === '' &&
      url.hash === '' &&
      /^\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\/?$/.test(url.pathname) &&
      url.pathname
        .split('/')
        .filter(Boolean)
        .every((part) => !['.', '..', '.git'].includes(part))
    );
  } catch {
    return false;
  }
}

/** The Agent's name: the repository it prepares, or the generic name without one. */
export function agentNameFor(repositoryUrl: string): string {
  if (repositoryUrl === '') return 'New agent';
  return new URL(repositoryUrl).pathname
    .split('/')
    .filter(Boolean)
    .at(-1)!
    .replace(/\.git$/, '')
    .slice(0, 64);
}

/** Parse task entry shorthand without URL's dot-segment normalization. */
export function parseRepository(value: string): string | undefined {
  const match =
    /^(?:https:\/\/github\.com\/|github\.com\/)?([A-Za-z0-9_.-]+)\/([A-Za-z0-9_.-]+)\/?$/.exec(
      value.trim(),
    );
  if (!match) return undefined;
  const owner = match[1]!;
  const repo = match[2]!.replace(/\.git$/, '');
  if ([owner, repo].some((part) => !part || ['.', '..', '.git'].includes(part))) return undefined;
  return `https://github.com/${owner}/${repo}`;
}

export function validRepositoryRef(value: string): boolean {
  return (
    new TextEncoder().encode(value).length <= 255 &&
    !value.startsWith('-') &&
    ![...value].some(
      (character) =>
        character.charCodeAt(0) < 33 ||
        character.charCodeAt(0) === 127 ||
        /\s/.test(character) ||
        '~^:?*[\\'.includes(character),
    ) &&
    !value.includes('..') &&
    !value.includes('@{')
  );
}
