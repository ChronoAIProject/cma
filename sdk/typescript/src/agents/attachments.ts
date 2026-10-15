import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import { request } from '../http.js';
import { command, query, segment, type CommandOptions } from '../resources.js';
import { object, text, integer } from '../schema.js';

/** A verified file path in the Agent's persisted home. */
export interface AgentAttachment {
  readonly id: string;
  readonly agentId: string;
  readonly name: string;
  readonly path: string;
  readonly size: number;
  readonly mediaType: string;
  readonly sha256: string;
}
/** File supplies its own name; a plain Blob requires an explicit name. */
export interface AttachmentUploadOptions extends CommandOptions {
  readonly name?: string;
}
const maximumBytes = 20 * 1024 * 1024;
const metadata = object({
  id: text,
  agent_id: text,
  name: text,
  path: text,
  size_bytes: integer,
  media_type: text,
  sha256: text,
});
function safeName(value: string): string {
  if (
    !value ||
    new TextEncoder().encode(value).length > 1024 ||
    value === '.' ||
    value === '..' ||
    /[\p{Cc}/\\]/u.test(value)
  )
    throw new ValidationError(
      'attachment name must be a file name without path separators or controls',
    );
  return Array.from(value, (c) => (/^[A-Za-z0-9._-]$/.test(c) ? c : '_'))
    .slice(0, 120)
    .join('')
    .replace(/^\./, '_');
}
/** Uploads one file without sending an Agent message or executing its contents. */
export class AgentAttachments {
  constructor(private readonly config: ResolvedConfig) {}

  async upload(
    agentId: string,
    file: Blob,
    options: AttachmentUploadOptions,
  ): Promise<AgentAttachment> {
    const path = `/api/v1/agents/${segment(agentId)}/attachments`;
    const controls = command(options);
    if (!(file instanceof Blob) || file.size > maximumBytes)
      throw new ValidationError('each attachment must be a Blob containing at most 20 MiB');
    const name =
      options.name ?? ('name' in file && typeof file.name === 'string' ? file.name : undefined);
    if (name === undefined) throw new ValidationError('a file name is required for a Blob upload');
    const expectedName = safeName(name);
    const media = file.type || 'application/octet-stream';
    if (media.length > 127 || !/^[A-Za-z0-9!#$&^_.+-]+\/[A-Za-z0-9!#$&^_.+-]+$/.test(media))
      throw new ValidationError(
        'attachment media type must be a bounded type/subtype without parameters',
      );
    const headers = new Headers(controls.headers);
    headers.set('Content-Type', media);
    const result = metadata(
      await request(this.config, query(path, { name }), {
        ...controls,
        method: 'POST',
        headers,
        rawBody: file,
        expectedStatus: 201,
      }),
    );
    if (
      !/^att_[a-f0-9]{64}$/.test(result.id) ||
      result.agent_id !== agentId ||
      result.name !== expectedName ||
      result.size_bytes !== file.size ||
      result.media_type !== media ||
      !/^[a-f0-9]{64}$/.test(result.sha256) ||
      result.path !== `/home/attachments/${agentId}/${result.id}/${expectedName}`
    )
      throw new ProtocolError('attachment receipt did not match the requested file and Agent');
    return {
      id: result.id,
      agentId,
      name: result.name,
      path: result.path,
      size: result.size_bytes,
      mediaType: result.media_type,
      sha256: result.sha256,
    };
  }
}
