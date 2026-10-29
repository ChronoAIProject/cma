import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { request, type OperationOptions } from './http.js';
import { input, query, read, segment } from './resources.js';
import {
  dictionary,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  text,
  type Output,
} from './schema.js';

export const BindingMediaArtifactSchema = object({
  attachment_index: integer,
  kind: enumeration('original', 'transcript', 'text_excerpt'),
  content_sha256: text,
  source_sha256: text,
  name: text,
  media_type: text,
  size_bytes: integer,
  text_sha256: optional(nullable(text)),
});
export type BindingMediaArtifact = Output<typeof BindingMediaArtifactSchema>;
export const BindingMediaSupplementSchema = object({
  gateway_binding_generation: optional(nullable(positiveInteger)),
  event_expires_at: positiveInteger,
  artifact: BindingMediaArtifactSchema,
  derived_text: text,
});
export type BindingMediaSupplement = Output<typeof BindingMediaSupplementSchema>;
const attachment = object({
  id: text,
  agent_id: text,
  name: text,
  path: text,
  size_bytes: integer,
  media_type: text,
  sha256: text,
});
export type BindingMediaAttachment = Output<typeof attachment>;
const supplement = object({
  command_id: text,
  response_id: text,
  response_url: text,
  status: text,
  queue_deadline: text,
  metadata: dictionary(text),
});
export type BindingMediaSupplementReceipt = Output<typeof supplement>;
export interface BindingMediaUpload {
  readonly name: string;
  readonly sha256: string;
  readonly event_expires_at: number;
  readonly gateway_binding_generation?: number;
}
const digest = /^[a-f0-9]{64}$/;
function base(binding: string, conversation: string, event: string, index: number) {
  if (!Number.isSafeInteger(index) || index < 0 || index > 255)
    throw new ValidationError('attachment index must be an unsigned byte');
  return `/api/v1/cmaeg/bindings/${segment(binding)}/conversations/${segment(conversation)}/events/${segment(event)}/media/${String(index)}`;
}
function receipt(value: unknown): BindingMediaAttachment {
  const result = attachment(value);
  if (
    !/^att_[a-f0-9]{64}$/.test(result.id) ||
    !/^[A-Za-z0-9_-]{1,128}$/.test(result.agent_id) ||
    !digest.test(result.sha256) ||
    result.size_bytes > 20 * 1024 * 1024 ||
    !/^[A-Za-z0-9_-][A-Za-z0-9._-]{0,119}$/.test(result.name) ||
    result.path !== `/home/attachments/${result.agent_id}/${result.id}/${result.name}`
  )
    throw new ProtocolError('invalid binding media attachment receipt');
  return result;
}
/** Original binding/event authority only. Retain the same slot, hashes and bytes on retry. */
export class BindingEventMedia {
  constructor(private readonly config: ResolvedConfig) {}
  async upload(
    binding: string,
    conversation: string,
    event: string,
    index: number,
    file: Blob,
    params: BindingMediaUpload,
    options: OperationOptions = {},
  ): Promise<BindingMediaAttachment> {
    const path = base(binding, conversation, event, index);
    if (
      !(file instanceof Blob) ||
      file.size > 20 * 1024 * 1024 ||
      !/^[A-Za-z0-9_-][A-Za-z0-9._-]{0,119}$/.test(params.name) ||
      !digest.test(params.sha256) ||
      !Number.isSafeInteger(params.event_expires_at) ||
      params.event_expires_at <= 0 ||
      (params.gateway_binding_generation !== undefined &&
        (!Number.isSafeInteger(params.gateway_binding_generation) ||
          params.gateway_binding_generation <= 0))
    )
      throw new ValidationError('invalid original event media upload');
    const media = file.type || 'application/octet-stream';
    if (media.length > 127 || !/^[A-Za-z0-9!#$&^_.+-]+\/[A-Za-z0-9!#$&^_.+-]+$/.test(media))
      throw new ValidationError('invalid media type');
    const headers = new Headers();
    headers.set('content-type', media);
    headers.set('content-length', String(file.size));
    const result = receipt(
      await request(this.config, query(`${path}/attachment`, { ...params }), {
        ...options,
        method: 'POST',
        headers,
        rawBody: file,
        expectedStatus: 201,
      }),
    );
    if (
      result.name !== params.name ||
      result.sha256 !== params.sha256 ||
      result.size_bytes !== file.size ||
      result.media_type !== media
    )
      throw new ProtocolError('binding media receipt differs from the original attachment');
    return result;
  }
  async get(
    binding: string,
    conversation: string,
    event: string,
    index: number,
    options: OperationOptions = {},
  ) {
    return receipt(
      await request(
        this.config,
        `${base(binding, conversation, event, index)}/attachment`,
        options,
      ),
    );
  }
  supplement(
    binding: string,
    conversation: string,
    event: string,
    index: number,
    body: BindingMediaSupplement,
    options: OperationOptions = {},
  ) {
    const path = base(binding, conversation, event, index);
    const value = input(BindingMediaSupplementSchema, body);
    if (
      value.artifact.attachment_index !== index ||
      new TextEncoder().encode(value.derived_text).length > 64 * 1024
    )
      throw new ValidationError('media supplement index or derived text exceeds its contract');
    return read(this.config, `${path}/supplement`, supplement, {
      ...options,
      method: 'POST',
      body: value,
      expectedStatus: 202,
    });
  }
}
