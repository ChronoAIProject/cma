import type { ReadOptions } from './client';
import type { AgentEnvironment } from '@cma/client';
import type { ReadRetryMetadata } from './readRetry';

export interface AgentFileEntry {
  readonly path: string;
  readonly name: string;
  readonly kind: 'file' | 'directory' | 'symlink';
  readonly size_bytes: number;
  readonly hidden: boolean;
}
export interface AgentFileContent {
  readonly path: string;
  readonly size_bytes: number;
  readonly sha256: string;
  readonly content_base64: string;
}
export interface AgentFileWrite {
  readonly path: string;
  readonly content_base64: string;
  readonly expected_sha256?: string;
  readonly directory?: boolean;
}
export interface AgentFileMutation {
  readonly path: string;
  readonly size_bytes?: number | null;
  readonly sha256?: string | null;
  readonly deleted?: boolean | null;
}
export interface AgentFilesClient {
  environment?(agent: string, options?: ReadOptions): Promise<AgentEnvironment>;
  list(agent: string, path: string, options?: ReadOptions): Promise<readonly AgentFileEntry[]>;
  read(agent: string, path: string, options?: ReadOptions): Promise<AgentFileContent>;
  download(agent: string, path: string, options?: ReadOptions): Promise<Blob>;
  write(agent: string, input: AgentFileWrite, options?: ReadOptions): Promise<AgentFileMutation>;
  remove(agent: string, path: string, recursive: boolean, options?: ReadOptions): Promise<void>;
}
export class AgentFileError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code?: string,
    readonly problem?: ReadRetryMetadata,
  ) {
    super(message);
    this.name = 'AgentFileError';
  }
  get retryable(): boolean | undefined {
    return this.problem?.retryable;
  }
  get retryAfterMs(): number | undefined {
    return this.problem?.retryAfterMs;
  }
  get requestId(): string | undefined {
    return this.problem?.requestId;
  }
}
export const MAX_FILE_BYTES = 32 * 1024 * 1024;
export const PREVIEW_BYTES = 1024 * 1024;

export function fileBytes(content: string): Uint8Array<ArrayBuffer> {
  const binary = atob(content);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
export function fileBase64(bytes: Uint8Array): string {
  let binary = '';
  for (let offset = 0; offset < bytes.length; offset += 8192)
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  return btoa(binary);
}
