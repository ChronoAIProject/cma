import type { CreationProfile } from '../create/client';
import type { AgentProfileContent, ProfileDraft } from '../profiles/client';
import { validateFatherDraft, type FatherDraft } from './fatherDraft';

export interface FatherReceipt {
  readonly version: 1;
  readonly owner: string;
  readonly draft: FatherDraft;
  readonly createKey: string;
  readonly publishKey: string;
  readonly document?: { readonly id: string; readonly draftVersion: number };
  readonly published?: { readonly revision: number; readonly digest: string; readonly at: string };
}

const key = (owner: string) => `cma:bot-father:${JSON.stringify([location.origin, owner])}`;
const positive = (value: unknown) => Number.isSafeInteger(value) && Number(value) > 0;
const validKey = (value: unknown) =>
  typeof value === 'string' && /^[\x21-\x7e]{1,200}$/.test(value);

export function readFatherReceipt(owner: string): FatherReceipt | undefined {
  const raw = sessionStorage.getItem(key(owner));
  if (raw === null) return undefined;
  const value = JSON.parse(raw) as FatherReceipt;
  if (
    value?.version !== 1 ||
    value.owner !== owner ||
    !validKey(value.createKey) ||
    !validKey(value.publishKey) ||
    (value.document !== undefined &&
      (typeof value.document.id !== 'string' ||
        value.document.id === '' ||
        !positive(value.document.draftVersion))) ||
    (value.published !== undefined &&
      (value.document === undefined ||
        !positive(value.published.revision) ||
        typeof value.published.digest !== 'string' ||
        typeof value.published.at !== 'string'))
  )
    throw new Error(
      'Saved profile creation could not be read. Restore browser storage to continue.',
    );
  validateFatherDraft(value.draft);
  return value;
}

export function retainFatherReceipt(value: FatherReceipt): void {
  sessionStorage.setItem(key(value.owner), JSON.stringify(value));
}

export function clearFatherReceipt(owner: string): void {
  sessionStorage.removeItem(key(owner));
}

export function fatherProfileDraft(draft: FatherDraft): ProfileDraft<AgentProfileContent> {
  return {
    name: draft.name,
    description: 'Created from a reviewed CMA Bot Father setup.',
    content: {
      thinkingEffort: 'medium',
      permissionLevel: 'guarded',
      skills: [],
      systemPrompt: draft.instructions,
    },
  };
}

export function fatherCreationProfile(value: FatherReceipt): CreationProfile | undefined {
  if (value.document === undefined || value.published === undefined) return undefined;
  return {
    kind: 'agent',
    name: value.draft.name,
    disclosures: [],
    requiresConfirmation: false,
    revision: {
      profileId: value.document.id,
      revision: value.published.revision,
      digest: value.published.digest,
      publishedAt: value.published.at,
      content: fatherProfileDraft(value.draft).content,
    },
  };
}
