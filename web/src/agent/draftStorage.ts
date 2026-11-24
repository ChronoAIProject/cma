import { restoreAttachments, storeAttachments, type StoredAttachment } from './draftAttachments';
import type { DraftAttachment } from './attachments';

export const agentDraftPrefix = 'cma:agent-draft:';
export const draftTextLimit = 256 * 1024;
export const draftStorageLimit = 1024 * 1024;
export const draftCountLimit = 6;
export type DraftStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem' | 'key' | 'length'>;
export interface DraftValue {
  readonly revision: string;
  readonly content: string;
  readonly attachments: readonly DraftAttachment[];
  readonly commandKey?: string;
}
interface StoredDraft {
  readonly version: 1;
  readonly owner: string;
  readonly agentId: string;
  readonly revision: string;
  readonly text: string;
  readonly attachments: readonly StoredAttachment[];
  readonly commandKey?: string;
}
export const emptyDraft: DraftValue = { revision: '', content: '', attachments: [] };
export const draftReadError =
  'Your saved Agent draft could not be recovered. Restore browser session storage and retry recovery, or discard the saved draft. Copy any current text first.';
export const draftWriteError =
  'Your draft was not saved to this browser. Copy your text, then restore session storage or free space in your other Agent drafts and retry saving.';
type DraftListener = (owner: string, agentId?: string, revision?: string, error?: string) => void;
const listeners = new Set<DraftListener>();

export function subscribeDrafts(listener: DraftListener) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
export function agentDraftKey(owner: string, agentId: string) {
  return `${agentDraftPrefix}${JSON.stringify([location.origin, owner, agentId])}`;
}
function bytes(value: string) {
  return new TextEncoder().encode(value).length;
}
function keys(storage: DraftStorage) {
  return Array.from({ length: storage.length }, (_, index) => storage.key(index)).filter(
    (key): key is string => key?.startsWith(agentDraftPrefix) === true,
  );
}
function parse(raw: string, owner: string, agentId: string): DraftValue {
  if (bytes(raw) > draftStorageLimit) throw new Error(draftReadError);
  const row = JSON.parse(raw) as StoredDraft | null;
  if (
    !row ||
    row.version !== 1 ||
    row.owner !== owner ||
    row.agentId !== agentId ||
    typeof row.revision !== 'string' ||
    !row.revision ||
    row.revision.length > 256 ||
    typeof row.text !== 'string' ||
    bytes(row.text) > draftTextLimit ||
    (row.commandKey !== undefined &&
      (typeof row.commandKey !== 'string' || !row.commandKey || row.commandKey.length > 256))
  )
    throw new Error(draftReadError);
  return {
    revision: row.revision,
    content: row.text,
    attachments: restoreAttachments(row.attachments, agentId),
    commandKey: row.commandKey,
  };
}
export function readAgentDraft(
  owner: string,
  agentId: string,
  storage: DraftStorage = sessionStorage,
): DraftValue {
  const raw = storage.getItem(agentDraftKey(owner, agentId));
  return raw === null ? emptyDraft : parse(raw, owner, agentId);
}
export function saveAgentDraft(
  owner: string,
  agentId: string,
  value: DraftValue,
  storage: DraftStorage = sessionStorage,
) {
  if (!owner || !agentId || bytes(value.content) > draftTextLimit) throw new Error(draftWriteError);
  const key = agentDraftKey(owner, agentId);
  if (value.content === '' && value.attachments.length === 0) {
    storage.removeItem(key);
    return;
  }
  const record: StoredDraft = {
    version: 1,
    owner,
    agentId,
    revision: value.revision,
    text: value.content,
    attachments: storeAttachments(value.attachments),
    ...(value.commandKey === undefined ? {} : { commandKey: value.commandKey }),
  };
  const raw = JSON.stringify(record);
  parse(raw, owner, agentId);
  const other = keys(storage).filter((entry) => entry !== key);
  const total = other.reduce(
    (size, entry) => size + bytes(entry) + bytes(storage.getItem(entry) ?? ''),
    bytes(key) + bytes(raw),
  );
  if (other.length >= draftCountLimit || total > draftStorageLimit)
    throw new Error(draftWriteError);
  storage.setItem(key, raw);
}
export function removeAgentDraft(
  owner: string,
  agentId: string,
  storage: DraftStorage = sessionStorage,
) {
  storage.removeItem(agentDraftKey(owner, agentId));
}

/** Import only into a readable empty destination; existing drafts are never replaced. */
export function importEmptyAgentDraft(
  owner: string,
  agentId: string,
  content: string,
  current: () => boolean,
  storage: DraftStorage = sessionStorage,
): DraftValue | undefined {
  const empty = (value: DraftValue) =>
    !value.content && value.attachments.length === 0 && !value.commandKey;
  if (!content || !current() || !empty(readAgentDraft(owner, agentId, storage))) return;
  const value: DraftValue = { revision: crypto.randomUUID(), content, attachments: [] };
  if (!current() || !empty(readAgentDraft(owner, agentId, storage))) return;
  saveAgentDraft(owner, agentId, value, storage);
  const saved = readAgentDraft(owner, agentId, storage);
  if (
    !current() ||
    saved.revision !== value.revision ||
    saved.content !== content ||
    saved.attachments.length !== 0 ||
    saved.commandKey !== undefined
  )
    throw new Error(draftWriteError);
  return saved;
}
export function clearOwnerDrafts(owner: string, storage: DraftStorage = sessionStorage) {
  // Invalidate mounted callbacks even when the browser refuses deletion.
  for (const notify of listeners) notify(owner);
  for (const key of keys(storage)) {
    let identity: unknown;
    try {
      identity = JSON.parse(key.slice(agentDraftPrefix.length));
    } catch {
      storage.removeItem(key);
      continue;
    }
    if (Array.isArray(identity) && identity[1] === owner) storage.removeItem(key);
  }
}
export function acceptDraftCommand(
  owner: string,
  agentId: string,
  commandKey: string,
  storage: DraftStorage,
) {
  let value: DraftValue | undefined;
  try {
    value = readAgentDraft(owner, agentId, storage);
    if (value.commandKey !== commandKey) return;
    removeAgentDraft(owner, agentId, storage);
    for (const notify of listeners) notify(owner, agentId, value.revision);
  } catch {
    if (value?.commandKey === commandKey)
      for (const notify of listeners)
        notify(
          owner,
          agentId,
          value.revision,
          'Your message was accepted, but its saved draft could not be removed. Restore browser storage and retry draft storage before reloading.',
        );
  }
}
