import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { requestBytes, type OperationOptions } from './http.js';
import { input, query, read, segment } from './resources.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from './schema.js';

const MAX_BYTES = 32 * 1024 * 1024;
const MAX_ENCODED_BYTES = Math.ceil(MAX_BYTES / 3) * 4;
export const agentFileEntry = object({
  path: text,
  name: text,
  kind: enumeration('file', 'directory', 'symlink'),
  size_bytes: integer,
  sha256: optional(text),
  hidden: boolean,
});
export const agentFileList = object({ path: text, entries: array(agentFileEntry) });
export const agentFileContent = object({
  path: text,
  size_bytes: integer,
  sha256: text,
  content_base64: text,
});
export const agentFileWrite = object({
  path: text,
  content_base64: text,
  expected_sha256: optional(text),
  directory: optional(boolean),
});
export const agentFileMutation = object({
  path: text,
  size_bytes: nullable(integer),
  sha256: nullable(text),
  deleted: nullable(boolean),
});
export type AgentFileEntry = Output<typeof agentFileEntry>;
export type AgentFileList = Output<typeof agentFileList>;
export type AgentFileContent = Output<typeof agentFileContent>;
export type AgentFileWrite = Output<typeof agentFileWrite>;
export type AgentFileMutation = Output<typeof agentFileMutation>;

/** Persistent /home files. Paths are relative; every request rechecks current access. */
export class AgentFiles {
  constructor(private readonly config: ResolvedConfig) {}
  /** List one directory lazily. An empty path names /home; dotfiles are included. */
  list(agentId: string, path = '', options: OperationOptions = {}) {
    return read(this.config, query(this.path(agentId, '/tree'), { path }), agentFileList, options);
  }
  /** Read bounded base64 content; omitted maxBytes uses the server's preview bound. */
  async read(
    agentId: string,
    path: string,
    options: OperationOptions & { maxBytes?: number } = {},
  ) {
    if (
      options.maxBytes !== undefined &&
      (!Number.isSafeInteger(options.maxBytes) ||
        options.maxBytes < 1 ||
        options.maxBytes > MAX_BYTES)
    )
      throw new ValidationError('maxBytes must be between 1 and 32 MiB');
    const bytes = await requestBytes(
      this.config,
      query(this.path(agentId, '/content'), { path, max_bytes: options.maxBytes }),
      { ...options, expectedStatus: 200 },
      MAX_ENCODED_BYTES + 4096,
    );
    let value: unknown;
    try {
      value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)) as unknown;
    } catch (cause) {
      throw new ProtocolError('file response was not valid JSON', { cause });
    }
    return agentFileContent(value);
  }
  /** Download at most 32 MiB as bytes without interpreting or executing file content. */
  download(agentId: string, path: string, options: OperationOptions = {}) {
    return requestBytes(
      this.config,
      query(this.path(agentId, '/download'), { path }),
      { ...options, expectedStatus: 200 },
      MAX_BYTES,
    );
  }
  /** Create-only without expected_sha256; otherwise compare-and-swap the last read version. */
  write(agentId: string, body: AgentFileWrite, options: OperationOptions = {}) {
    input(agentFileWrite, body);
    if (body.content_base64.length > MAX_ENCODED_BYTES)
      throw new ValidationError('file content exceeds 32 MiB');
    return read(this.config, this.path(agentId), agentFileMutation, {
      ...options,
      method: 'POST',
      body,
    });
  }
  /** Remove exactly one path. Recursive directory removal must be explicit. */
  remove(agentId: string, path: string, recursive = false, options: OperationOptions = {}) {
    return read(
      this.config,
      query(this.path(agentId), { path, recursive: recursive ? 'true' : undefined }),
      agentFileMutation,
      { ...options, method: 'DELETE' },
    );
  }
  private path(agentId: string, suffix = '') {
    return `/api/v1/agents/${segment(agentId)}/files${suffix}`;
  }
}
