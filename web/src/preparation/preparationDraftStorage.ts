import { draftCountLimit, draftStorageLimit, draftTextLimit } from '../agent/draftStorage';
import { preparationDraftPrefix, requirePreparationDraftOwner } from './preparationDraftIdentity';

export interface PreparationDraftScope {
  readonly owner: string;
  readonly intentKey: string;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
}
export interface PreparationDraftBinding {
  readonly operationId: string;
  readonly agentId: string;
  readonly sandboxId: string;
  readonly workspaceId: string;
  readonly worktreeId: string;
}
export interface PreparationDraftRecord {
  readonly version: 1;
  readonly scope: PreparationDraftScope;
  readonly revision: string;
  readonly text: string;
  readonly binding?: PreparationDraftBinding;
}
export function samePreparationBinding(
  left: PreparationDraftBinding | undefined,
  right: PreparationDraftBinding | undefined,
) {
  return (
    left === right ||
    (left !== undefined &&
      right !== undefined &&
      left.operationId === right.operationId &&
      left.agentId === right.agentId &&
      left.sandboxId === right.sandboxId &&
      left.workspaceId === right.workspaceId &&
      left.worktreeId === right.worktreeId)
  );
}
export const preparationDraftReadError =
  'Your saved preparation draft could not be read. Restore browser storage and retry. Your original task request is unchanged.';
export const preparationDraftWriteError =
  'This draft could not be saved. Copy your text before reloading, then restore browser storage or free space and retry saving.';
const bytes = (value: string) => new TextEncoder().encode(value).length;
const identifier = (value: unknown): value is string =>
  typeof value === 'string' && value.length > 0 && value.length <= 256;
const same = (left: unknown, right: unknown) => JSON.stringify(left) === JSON.stringify(right);

export function preparationDraftKey(scope: PreparationDraftScope) {
  return (
    preparationDraftPrefix +
    JSON.stringify([
      location.origin,
      scope.owner,
      scope.workspaceId ?? null,
      scope.sandboxId ?? null,
      scope.intentKey,
    ])
  );
}
function parse(raw: string, scope: PreparationDraftScope): PreparationDraftRecord {
  if (bytes(raw) > draftStorageLimit) throw new Error(preparationDraftReadError);
  const row = JSON.parse(raw) as PreparationDraftRecord | null;
  if (
    !row ||
    row.version !== 1 ||
    !identifier(scope.owner) ||
    !identifier(scope.intentKey) ||
    !row.scope ||
    row.scope.owner !== scope.owner ||
    row.scope.intentKey !== scope.intentKey ||
    row.scope.workspaceId !== scope.workspaceId ||
    row.scope.sandboxId !== scope.sandboxId ||
    !identifier(row.revision) ||
    typeof row.text !== 'string' ||
    bytes(row.text) > draftTextLimit ||
    (row.binding !== undefined &&
      (!row.binding ||
        !identifier(row.binding.operationId) ||
        !identifier(row.binding.agentId) ||
        !identifier(row.binding.sandboxId) ||
        !identifier(row.binding.workspaceId) ||
        !identifier(row.binding.worktreeId) ||
        (scope.workspaceId !== undefined && scope.workspaceId !== row.binding.workspaceId) ||
        (scope.sandboxId !== undefined && scope.sandboxId !== row.binding.sandboxId)))
  )
    throw new Error(preparationDraftReadError);
  return row;
}
export function readPreparationDraft(
  scope: PreparationDraftScope,
  storage: Storage = sessionStorage,
) {
  requirePreparationDraftOwner(scope.owner, storage);
  const raw = storage.getItem(preparationDraftKey(scope));
  return raw === null ? undefined : parse(raw, scope);
}
function compare(
  scope: PreparationDraftScope,
  expected: PreparationDraftRecord | undefined,
  storage: Storage,
) {
  if (!same(readPreparationDraft(scope, storage), expected))
    throw new Error(
      'This preparation draft changed elsewhere. Keep your text and reload before replacing it.',
    );
}
export function savePreparationDraft(
  scope: PreparationDraftScope,
  expected: PreparationDraftRecord | undefined,
  text: string,
  binding = expected?.binding,
  storage: Storage = sessionStorage,
) {
  compare(scope, expected, storage);
  if (expected?.binding && !samePreparationBinding(expected.binding, binding))
    throw new Error('Preparation Agent binding changed.');
  const next: PreparationDraftRecord = {
    version: 1,
    scope,
    revision: crypto.randomUUID(),
    text,
    ...(binding ? { binding } : {}),
  };
  const raw = JSON.stringify(next);
  parse(raw, scope);
  const key = preparationDraftKey(scope);
  const others = Array.from({ length: storage.length }, (_, index) => storage.key(index)).filter(
    (entry): entry is string =>
      entry !== null && entry.startsWith(preparationDraftPrefix) && entry !== key,
  );
  const total = others.reduce(
    (size, entry) => size + bytes(entry) + bytes(storage.getItem(entry) ?? ''),
    bytes(key) + bytes(raw),
  );
  if (others.length >= draftCountLimit || total > draftStorageLimit)
    throw new Error(preparationDraftWriteError);
  compare(scope, expected, storage);
  storage.setItem(key, raw);
  if (!same(readPreparationDraft(scope, storage), next))
    throw new Error(preparationDraftWriteError);
  return next;
}
export function removePreparationDraft(
  scope: PreparationDraftScope,
  expected: PreparationDraftRecord,
  storage: Storage = sessionStorage,
) {
  compare(scope, expected, storage);
  storage.removeItem(preparationDraftKey(scope));
  if (readPreparationDraft(scope, storage) !== undefined)
    throw new Error(preparationDraftWriteError);
}

export function preparationDraftsForAgent(
  owner: string,
  agentId: string,
  storage: Storage = sessionStorage,
) {
  requirePreparationDraftOwner(owner, storage);
  const result: PreparationDraftRecord[] = [];
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (!key?.startsWith(preparationDraftPrefix)) continue;
    const identity: unknown = JSON.parse(key.slice(preparationDraftPrefix.length));
    if (
      !Array.isArray(identity) ||
      identity.length !== 5 ||
      identity[0] !== location.origin ||
      identity[1] !== owner
    )
      continue;
    const scope: PreparationDraftScope = {
      owner,
      intentKey: identity[4],
      ...(identity[2] !== null ? { workspaceId: identity[2] } : {}),
      ...(identity[3] !== null ? { sandboxId: identity[3] } : {}),
    };
    const record = readPreparationDraft(scope, storage);
    if (record?.binding?.agentId === agentId) result.push(record);
  }
  return result;
}
