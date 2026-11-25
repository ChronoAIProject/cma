import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import {
  readBytes,
  readJsonBody,
  requestResponse,
  requireSuccess,
  send,
  withDeadline,
  type OperationOptions,
} from '../http.js';
import {
  command,
  input,
  page,
  query,
  read,
  segment,
  type CommandOptions,
  type PageOptions,
} from '../resources.js';
import { enumeration, object, positiveInteger, text, type Output } from '../schema.js';

const maximumBytes = 1024 * 1024;
const artifactId = (value: unknown): string => {
  const id = text(value);
  if (!/^art_[a-f0-9]{64}$/.test(id)) throw new ProtocolError('invalid artifact identity');
  return id;
};
const name = (value: unknown): string => {
  const result = text(value);
  if (
    result.trim() !== result ||
    result === '' ||
    result === '.' ||
    result === '..' ||
    new TextEncoder().encode(result).length > 160 ||
    /[\p{Cc}/\\]/u.test(result)
  )
    throw new ProtocolError('artifact name must be a bounded display name without path separators');
  return result;
};
const mediaType = enumeration(
  'text/plain',
  'text/markdown',
  'application/json',
  'application/pdf',
  'application/octet-stream',
  'image/png',
  'image/jpeg',
  'image/webp',
);
const content = (value: unknown) => {
  const encoded = text(value);
  if (encoded.length === 0 || encoded.length > Math.ceil(maximumBytes / 3) * 4)
    throw new ProtocolError('artifact content must contain 1 byte through 1 MiB');
  try {
    const decoded = atob(encoded);
    if (decoded.length === 0 || decoded.length > maximumBytes || btoa(decoded) !== encoded)
      throw new Error('noncanonical content');
    const bytes = new Uint8Array(decoded.length);
    for (let i = 0; i < bytes.length; i++) bytes[i] = decoded.charCodeAt(i);
    return { encoded, bytes };
  } catch (cause) {
    throw new ProtocolError('artifact content must be canonical standard base64 within 1 MiB', {
      cause,
    });
  }
};
const publishRequest = object({ name, media_type: mediaType, content_base64: content });
/** Explicitly selected content published to one Agent's current and future readers. */
export type PublishAgentArtifactRequest = Omit<Output<typeof publishRequest>, 'content_base64'> & {
  readonly content_base64: string;
};
const metadata = object({
  id: artifactId,
  agent_id: text,
  name,
  media_type: mediaType,
  size_bytes: positiveInteger,
  sha256: text,
  created_at: text,
});
/** Immutable approved content metadata; never a filesystem path or credential-bearing URL. */
export type AgentArtifactMetadata = Output<typeof metadata>;

function artifact(value: unknown, agentId: string, id?: string): AgentArtifactMetadata {
  const result = metadata(value);
  if (
    result.agent_id !== agentId ||
    (id !== undefined && result.id !== id) ||
    result.size_bytes > maximumBytes ||
    !/^[a-f0-9]{64}$/.test(result.sha256) ||
    !Number.isFinite(Date.parse(result.created_at))
  )
    throw new ProtocolError('artifact metadata did not match the requested content');
  return result;
}
function collection(agentId: string): string {
  return `/api/v1/agents/${segment(agentId)}/artifacts`;
}

export function listAgentArtifacts(
  config: ResolvedConfig,
  agentId: string,
  options: PageOptions = {},
) {
  if (options.cursor !== undefined) input(artifactId, options.cursor);
  if (options.limit !== undefined)
    input((value) => {
      const limit = positiveInteger(value);
      if (limit > 50) throw new ProtocolError('artifact page limit must be at most 50');
      return limit;
    }, options.limit);
  return read(
    config,
    query(collection(agentId), { cursor: options.cursor, limit: options.limit }),
    (value) => {
      const result = page((item) => artifact(item, agentId))(value);
      if (result.items.length > (options.limit ?? 50))
        throw new ProtocolError('artifact page exceeded its limit');
      let previous = options.cursor;
      for (const item of result.items) {
        if (previous !== undefined && item.id <= previous)
          throw new ProtocolError('artifact page order did not advance');
        previous = item.id;
      }
      if (
        result.next !== undefined &&
        (artifactId(result.next) !== previous || result.items.length === 0)
      )
        throw new ProtocolError('artifact continuation did not match the page');
      return result;
    },
    options,
  );
}
export function readAgentArtifact(
  config: ResolvedConfig,
  agentId: string,
  id: string,
  options: OperationOptions = {},
) {
  input(artifactId, id);
  return read(
    config,
    `${collection(agentId)}/${segment(id)}`,
    (value) => artifact(value, agentId, id),
    options,
  );
}
export function publishAgentArtifact(
  config: ResolvedConfig,
  agentId: string,
  body: PublishAgentArtifactRequest,
  options: CommandOptions,
) {
  let selected: Output<typeof publishRequest>;
  try {
    selected = publishRequest(body);
  } catch (cause) {
    throw new ValidationError(cause instanceof Error ? cause.message : 'invalid request body', {
      cause,
    });
  }
  const path = collection(agentId);
  const { encoded, bytes } = selected.content_base64;
  // The transport can yield to caller callbacks before serializing the body.
  const snapshot = {
    name: selected.name,
    media_type: selected.media_type,
    content_base64: encoded,
  };
  const request = { ...command(options), method: 'POST', body: snapshot };
  return withDeadline(config, request, async (signal, requestId) => {
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
    const sha256 = Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join('');
    const { response, metadata } = await send(config, path, request, signal, requestId);
    await requireSuccess(response, 201, signal, metadata.requestId);
    const result = artifact(await readJsonBody(response, signal), agentId);
    if (
      result.name !== snapshot.name ||
      result.media_type !== snapshot.media_type ||
      result.size_bytes !== bytes.length ||
      result.sha256 !== sha256
    ) {
      throw new ProtocolError(
        'The publication receipt did not match the selected artifact. Retry to check the same request.',
      );
    }
    return result;
  });
}
export function downloadAgentArtifact(
  config: ResolvedConfig,
  agentId: string,
  id: string,
  options: OperationOptions = {},
) {
  input(artifactId, id);
  return withDeadline(config, options, async (signal, requestId) => {
    const response = await requestResponse(
      config,
      `${collection(agentId)}/${segment(id)}/download`,
      { ...options, signal, requestId, expectedStatus: 200 },
    );
    if (
      response.headers.get('content-type')?.split(';')[0]?.trim().toLowerCase() !==
      'application/octet-stream'
    ) {
      await response.body?.cancel();
      throw new ProtocolError('artifact download did not return opaque bytes');
    }
    const bytes = await readBytes(response, signal);
    if (bytes.length === 0 || bytes.length > maximumBytes)
      throw new ProtocolError('artifact download exceeded its content bounds');
    return bytes;
  });
}
