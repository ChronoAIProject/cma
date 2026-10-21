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
