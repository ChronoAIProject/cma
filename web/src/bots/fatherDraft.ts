/** An untrusted suggestion, never an identity, permission grant, or creation command. */
export interface FatherDraft {
  readonly v: 1;
  readonly name: string;
  readonly instructions: string;
}

const pendingKey = 'cma:bot-father-draft:v1';
const bytes = (value: string) => new TextEncoder().encode(value).length;

export function validateFatherDraft(value: unknown): FatherDraft {
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    throw new Error('The Bot Father setup link is invalid. Ask for a new setup link.');
  const draft = value as Record<string, unknown>;
  if (
    Object.keys(draft).sort().join(',') !== 'instructions,name,v' ||
    draft.v !== 1 ||
    typeof draft.name !== 'string' ||
    draft.name.trim() === '' ||
    bytes(draft.name) > 80 ||
    /\p{Cc}/u.test(draft.name) ||
    typeof draft.instructions !== 'string' ||
    draft.instructions.trim() === '' ||
    bytes(draft.instructions) > 4096 ||
    Array.from(draft.instructions).some(
      (character) => /\p{Cc}/u.test(character) && !'\t\n\r'.includes(character),
    )
  )
    throw new Error('Use a name of at most 80 bytes and instructions of at most 4 KB.');
  return { v: 1, name: draft.name, instructions: draft.instructions };
}

export function parseFatherDraft(search: string): FatherDraft | undefined {
  if (!new URLSearchParams(search).has('setup')) return undefined;
  if (search.length > 25000) throw new Error('The Bot Father setup link is too long.');
  const values = new URLSearchParams(search).getAll('setup');
  if (values.length !== 1 || (values[0]?.length ?? 0) > 5000)
    throw new Error('The Bot Father setup link is invalid.');
  try {
    return validateFatherDraft(JSON.parse(values[0] ?? ''));
  } catch {
    throw new Error('The Bot Father setup link is invalid. Ask for a new setup link.');
  }
}

/** Called only when the visitor chooses sign-in; OAuth returns to the clean local path. */
export function retainFatherDraftForSignIn(): void {
  if (window.location.pathname !== '/bots/new') return;
  const draft = parseFatherDraft(window.location.search);
  if (draft !== undefined) sessionStorage.setItem(pendingKey, JSON.stringify(draft));
}

export function readFatherDraft(): FatherDraft | undefined {
  const linked = parseFatherDraft(window.location.search);
  if (linked !== undefined) return linked;
  const saved = sessionStorage.getItem(pendingKey);
  return saved === null ? undefined : validateFatherDraft(JSON.parse(saved));
}

export function clearFatherDraft(): void {
  sessionStorage.removeItem(pendingKey);
  if (window.location.pathname === '/bots/new') {
    const url = new URL(window.location.href);
    url.searchParams.delete('setup');
    window.history.replaceState(null, '', `${url.pathname}${url.search}${url.hash}`);
  }
}
