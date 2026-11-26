import { channelDraft, object, without, type Fields } from './model';

const storageKey = 'cma:event-gateway:new-channel:v1';
const recordId = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const draftKeys = [
  'schema_version',
  'name',
  'provider',
  'profile',
  'sources',
  'partition',
  'reply',
  'connection_ref',
  'connection_admission',
];

export interface NewChannelDraft {
  readonly fields: Fields;
  readonly open: boolean;
  readonly privateFields: boolean;
  readonly record?: string;
}

function draftSettings(fields: Fields): Fields {
  const profile = object(fields.profile);
  return {
    ...Object.fromEntries(
      draftKeys.filter((key) => Object.hasOwn(fields, key)).map((key) => [key, fields[key]]),
    ),
    profile: {
      ...without(profile, ['environment']),
      context: { ...without(object(profile.context), ['mcp_servers']), mcp_servers: [] },
    },
  };
}

function storedDraft(): Fields {
  return object(JSON.parse(sessionStorage.getItem(storageKey) ?? 'null'));
}

export function readNewChannelDraft(owner: string): NewChannelDraft | undefined {
  if (!owner) return;
  try {
    const saved = storedDraft();
    const fields = object(saved.fields);
    if (saved.owner !== owner || typeof fields.name !== 'string' || !Array.isArray(fields.sources))
      return;
    return {
      fields: { ...channelDraft(undefined, owner), ...draftSettings(fields) },
      open: saved.open === true,
      privateFields: saved.privateFields === true,
      record:
        typeof saved.record === 'string' && recordId.test(saved.record) ? saved.record : undefined,
    };
  } catch {
    // Storage can be unavailable. The in-memory form and unload warning still work.
    return;
  }
}

export function writeNewChannelDraft(
  owner: string,
  fields: Fields,
  privateFields: boolean,
  record?: string,
) {
  if (!owner) return;
  try {
    const profile = object(fields.profile);
    const servers = object(profile.context).mcp_servers;
    const omitted = profile.environment != null || (Array.isArray(servers) && servers.length > 0);
    sessionStorage.setItem(
      storageKey,
      JSON.stringify({
        owner,
        fields: draftSettings(fields),
        open: true,
        privateFields: privateFields || omitted,
        record,
      }),
    );
  } catch {
    // Fall back to the in-memory form and unload warning.
  }
}

export function setNewChannelDraftOpen(owner: string, open: boolean) {
  try {
    const saved = storedDraft();
    if (saved.owner === owner)
      sessionStorage.setItem(storageKey, JSON.stringify({ ...saved, open }));
  } catch {
    // No recoverable draft is available in this browser.
  }
}

export function clearNewChannelDraft(owner?: string, record?: string) {
  try {
    const saved = owner ? storedDraft() : undefined;
    if (!owner || (saved?.owner === owner && saved.record === record))
      sessionStorage.removeItem(storageKey);
  } catch {
    // A storage failure must not keep the protected view mounted.
  }
}

export function releaseNewChannelDraft(owner: string, record: string) {
  try {
    const saved = storedDraft();
    if (saved.owner === owner && saved.record === record)
      sessionStorage.setItem(storageKey, JSON.stringify(without(saved, ['record'])));
  } catch {
    // The original request remains recoverable in memory.
  }
}
