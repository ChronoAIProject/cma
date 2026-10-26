import { ProtocolError } from '@chronoai/cma-sdk';
import { AgentFileError, MAX_FILE_BYTES, PREVIEW_BYTES } from '../agentFiles';
import type {
  AgentFileContent,
  AgentFileEntry,
  AgentFileMutation,
  AgentFilesClient,
} from '../agentFiles';

function route(agent: string, suffix: string, path?: string): string {
  return `/api/v1/agents/${encodeURIComponent(agent)}/files${suffix}${path === undefined ? '' : `?${new URLSearchParams({ path })}`}`;
}
async function request(url: string, init: RequestInit): Promise<Response> {
  const response = await fetch(url, { credentials: 'same-origin', cache: 'no-store', ...init });
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as { detail?: unknown; code?: unknown };
    throw new AgentFileError(
      typeof body.detail === 'string' ? body.detail : 'Files could not be accessed. Try again.',
      response.status,
      typeof body.code === 'string' ? body.code : undefined,
    );
  }
  return response;
}
function validPath(path: unknown): path is string {
  return (
    typeof path === 'string' &&
    path.length <= 1024 &&
    !path.includes('\\') &&
    ![...path].some((character) => character.charCodeAt(0) < 32) &&
    (path === '' || path.split('/').every((part) => part !== '' && part !== '.' && part !== '..'))
  );
}
function validDigest(value: unknown): value is string {
  return typeof value === 'string' && /^[a-f0-9]{64}$/i.test(value);
}
function decodedSize(value: string): number {
  if (value.length % 4 !== 0 || /[^A-Za-z0-9+/=]/.test(value)) return -1;
  const padding = value.endsWith('==') ? 2 : value.endsWith('=') ? 1 : 0;
  if (value.indexOf('=') !== (padding === 0 ? -1 : value.length - padding)) return -1;
  return (value.length / 4) * 3 - padding;
}
export function agentFilesClient(): AgentFilesClient {
  return {
    async list(agent, path, options) {
      const response = await request(route(agent, '/tree', path), { signal: options?.signal });
      const value = (await response.json()) as { path: string; entries: AgentFileEntry[] };
      if (
        value.path !== path ||
        !Array.isArray(value.entries) ||
        value.entries.length > 10_000 ||
        value.entries.some(
          (entry) =>
            !validPath(entry.path) ||
            !entry.name ||
            entry.path !== (path ? `${path}/` : '') + entry.name ||
            entry.name.includes('/') ||
            !['file', 'directory', 'symlink'].includes(entry.kind) ||
            typeof entry.hidden !== 'boolean' ||
            !Number.isSafeInteger(entry.size_bytes) ||
            entry.size_bytes < 0,
        ) ||
        new Set(value.entries.map((entry) => entry.path)).size !== value.entries.length
      )
        throw new ProtocolError('Agent Manager returned an invalid directory listing.');
      return value.entries;
    },
    async read(agent, path, options) {
      const response = await request(
        `${route(agent, '/content', path)}&max_bytes=${PREVIEW_BYTES}`,
        { signal: options?.signal },
      );
      const value = (await response.json()) as AgentFileContent;
      if (
        value.path !== path ||
        !validDigest(value.sha256) ||
        !Number.isSafeInteger(value.size_bytes) ||
        value.size_bytes < 0 ||
        value.size_bytes > PREVIEW_BYTES ||
        typeof value.content_base64 !== 'string' ||
        value.content_base64.length > Math.ceil(PREVIEW_BYTES / 3) * 4 ||
        decodedSize(value.content_base64) !== value.size_bytes
      )
        throw new ProtocolError('Agent Manager returned an invalid file preview.');
      return value;
    },
    async download(agent, path, options) {
      const response = await request(route(agent, '/download', path), { signal: options?.signal });
      const blob = await response.blob();
      if (blob.size > MAX_FILE_BYTES)
        throw new ProtocolError('The file exceeds the download limit.');
      return blob;
    },
    async write(agent, input, options) {
      const response = await request(route(agent, ''), {
        method: 'POST',
        signal: options?.signal,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(input),
      });
      const value = (await response.json()) as AgentFileMutation;
      if (
        value.path !== input.path ||
        !validDigest(value.sha256) ||
        value.size_bytes !== (input.directory ? 0 : decodedSize(input.content_base64)) ||
        value.deleted === true
      )
        throw new ProtocolError('Agent Manager did not confirm this file save.');
      return value;
    },
    async remove(agent, path, recursive, options) {
      const response = await request(`${route(agent, '', path)}&recursive=${String(recursive)}`, {
        method: 'DELETE',
        signal: options?.signal,
      });
      const value = (await response.json()) as AgentFileMutation;
      if (value.path !== path || value.deleted !== true)
        throw new ProtocolError('Agent Manager did not confirm this file deletion.');
    },
  };
}
