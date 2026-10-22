const prefix = 'cma:document-share:';
const valid = (value: string | null): value is string =>
  value !== null && /^[a-f0-9]{64}$/.test(value);

export function documentShareUrl(id: string, token: string): string {
  if (!/^doc_[a-f0-9]{32}$/.test(id) || !valid(token)) throw new Error('Invalid document link.');
  return `${window.location.origin}/artifacts/${id}#token=${token}`;
}

export function readDocumentShareToken(id: string): string | undefined {
  const fragment = new URLSearchParams(window.location.hash.slice(1)).get('token');
  if (valid(fragment)) return fragment;
  try {
    const stored: unknown = JSON.parse(sessionStorage.getItem(prefix + id) ?? 'null');
    if (
      stored &&
      typeof stored === 'object' &&
      'token' in stored &&
      typeof stored.token === 'string' &&
      valid(stored.token) &&
      'expiresAt' in stored &&
      typeof stored.expiresAt === 'number' &&
      stored.expiresAt > Date.now()
    )
      return stored.token;
  } catch {
    /* A blocked store does not grant access. */
  }
  return undefined;
}

/** Bearer authority stays in this tab, never in OAuth URLs, server queries or referrers. */
export function retainDocumentShareToken(id: string, requireStorage = false): string | undefined {
  const token = readDocumentShareToken(id);
  if (!token) return undefined;
  try {
    sessionStorage.setItem(
      prefix + id,
      JSON.stringify({ token, expiresAt: Date.now() + 30 * 60 * 1000 }),
    );
  } catch {
    if (requireStorage)
      throw new Error('Allow session storage, then reopen the document link to sign in.');
  }
  if (window.location.hash.startsWith('#token='))
    window.history.replaceState(
      window.history.state,
      '',
      window.location.pathname + window.location.search,
    );
  return token;
}
