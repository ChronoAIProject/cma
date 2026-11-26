import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import type { OperationOptions, RequestOptions } from './http.js';
import {
  command,
  identity,
  input,
  query,
  read,
  segment,
  type CommandOptions,
} from './resources.js';
import {
  array,
  enumeration,
  nullable,
  object,
  optional,
  positiveInteger,
  text,
  type Output,
  type Parser,
} from './schema.js';

const permission = enumeration('read', 'comment');
const metadata = object({
  id: text,
  owner_subject: text,
  title: text,
  revision: positiveInteger,
  source_revision: positiveInteger,
  sha256: text,
  size_bytes: positiveInteger,
  created_at: text,
  updated_at: text,
  origin_agent_id: nullable(text),
  access: enumeration('owner', 'read', 'comment'),
});
const revision = object({
  revision: positiveInteger,
  title: text,
  sha256: text,
  size_bytes: positiveInteger,
  created_at: text,
  author_subject: text,
});
const deletionReceipt = object({ id: text, revision: positiveInteger, deleted_at: text });
const source = object({ id: text, revision: positiveInteger, markdown: text });
const comment = object({
  id: text,
  author_subject: text,
  body: text,
  document_revision: positiveInteger,
  created_at: text,
});
const share = object({ subject: text, permission });
const shareRecipient = object({
  subject: (value) => {
    const subject = text(value);
    if (!/^[ -~]{1,255}$/.test(subject) || subject.trim() !== subject)
      throw new ProtocolError('Invalid document share recipient subject');
    return subject;
  },
  display_name: nullable(text),
});
const shareRecipients = (value: unknown) => {
  if (!Array.isArray(value) || value.length > 20)
    throw new ProtocolError('Invalid document share recipient results');
  return array(shareRecipient)(value);
};
const link = object({ id: text, permission, created_at: text });
const mutation = object({ document: metadata, comment: optional(comment), link: optional(link) });
const create = object({ title: text, markdown: text, origin_agent_id: optional(nullable(text)) });
const update = object({
  expected_revision: positiveInteger,
  title: optional(nullable(text)),
  markdown: optional(nullable(text)),
});
const expected = object({ expected_revision: positiveInteger });
const grant = object({ expected_revision: positiveInteger, permission });
const createLink = object({ expected_revision: positiveInteger, permission, token: text });
const createComment = object({
  body: text,
  document_revision: optional(nullable(positiveInteger)),
});
const page = <T>(parse: Parser<T>) => object({ items: array(parse), next: nullable(text) });

/** Persistent document independent of its originating Agent or Sandbox. */
export type Document = Output<typeof metadata>;
/** One immutable Markdown source revision. */
export type DocumentRevision = Output<typeof revision>;
/** The recorded outcome of deleting an owned document. */
export type DocumentDeletionReceipt = Output<typeof deletionReceipt>;
/** Full, authorized Markdown source. */
export type DocumentSource = Output<typeof source>;
/** A comment anchored to one source revision. */
export type DocumentComment = Output<typeof comment>;
/** An explicit recipient grant. */
export type DocumentShare = Output<typeof share>;
export type DocumentShareRecipient = Output<typeof shareRecipient>;
/** Share-link metadata never includes the secret token. */
export type DocumentLink = Output<typeof link>;
/** An admitted document mutation. */
export type DocumentMutation = Output<typeof mutation>;
export type CreateDocument = Output<typeof create>;
export type UpdateDocument = Output<typeof update>;
export type GrantDocument = Output<typeof grant>;
export type CreateDocumentLink = Output<typeof createLink>;
export type CreateDocumentComment = Output<typeof createComment>;
export type DocumentRevisionRequest = Output<typeof expected>;
export interface DocumentAccessOptions extends OperationOptions {
  readonly shareToken?: string;
}
export interface DocumentPageOptions extends DocumentAccessOptions {
  readonly limit?: number;
  readonly cursor?: string;
}
export interface DocumentListOptions extends DocumentPageOptions {
  readonly scope?: 'all' | 'owned' | 'shared';
  readonly q?: string;
}
export type DocumentCommandOptions = CommandOptions & DocumentAccessOptions;

/** Owner registry, direct recipient access, and authenticated secret-link access. */
export class Documents {
  constructor(private readonly config: ResolvedConfig) {}
  list(options: DocumentListOptions = {}) {
    return read(
      this.config,
      query('/api/v1/documents', {
        limit: options.limit,
        cursor: options.cursor,
        scope: options.scope,
        q: options.q,
      }),
      page(metadata),
      options,
    );
  }
  read(id: string, options: DocumentAccessOptions = {}) {
    return this.get(id, '', metadata, options);
  }
  source(id: string, revision?: number, options: DocumentAccessOptions = {}) {
    return read(
      this.config,
      query(this.path(id, '/source'), { revision }),
      source,
      this.access(options),
    );
  }
  revisions(id: string, options: DocumentPageOptions = {}) {
    return this.paged(id, '/revisions', revision, options);
  }
  comments(id: string, options: DocumentPageOptions = {}) {
    return this.paged(id, '/comments', comment, options);
  }
  shares(id: string, options: OperationOptions = {}) {
    return this.get(id, '/shares', array(share), options);
  }
  shareRecipients(id: string, text?: string, options: OperationOptions = {}) {
    return read(
      this.config,
      query(this.path(id, '/share-recipients'), { q: text }),
      shareRecipients,
      options,
    );
  }
  links(id: string, options: OperationOptions = {}) {
    return this.get(id, '/share-links', array(link), options);
  }
  create(body: CreateDocument, options: CommandOptions) {
    input(create, body);
    return read(this.config, '/api/v1/documents', metadata, {
      ...command(options),
      method: 'POST',
      body,
      expectedStatus: 201,
    });
  }
  update(id: string, body: UpdateDocument, options: CommandOptions) {
    return this.write(id, '', 'PATCH', input(update, body), metadata, options);
  }
  delete(id: string, body: DocumentRevisionRequest, options: CommandOptions) {
    return this.write(
      id,
      '',
      'DELETE',
      input(expected, body),
      identity(deletionReceipt, id),
      options,
    );
  }
  grant(id: string, subject: string, body: GrantDocument, options: CommandOptions) {
    return this.write(
      id,
      `/shares/${segment(subject)}`,
      'PUT',
      input(grant, body),
      metadata,
      options,
    );
  }
  revoke(id: string, subject: string, body: DocumentRevisionRequest, options: CommandOptions) {
    return this.write(
      id,
      `/shares/${segment(subject)}`,
      'DELETE',
      input(expected, body),
      metadata,
      options,
    );
  }
  createLink(id: string, body: CreateDocumentLink, options: CommandOptions) {
    return this.write(id, '/share-links', 'POST', input(createLink, body), mutation, options);
  }
  revokeLink(id: string, linkId: string, body: DocumentRevisionRequest, options: CommandOptions) {
    return this.write(
      id,
      `/share-links/${segment(linkId)}`,
      'DELETE',
      input(expected, body),
      metadata,
      options,
    );
  }
  comment(id: string, body: CreateDocumentComment, options: DocumentCommandOptions) {
    return this.write(id, '/comments', 'POST', input(createComment, body), mutation, options);
  }
  removeComment(
    id: string,
    commentId: string,
    body: DocumentRevisionRequest,
    options: DocumentCommandOptions,
  ) {
    return this.write(
      id,
      `/comments/${segment(commentId)}`,
      'DELETE',
      input(expected, body),
      metadata,
      options,
    );
  }
  private path(id: string, suffix: string) {
    return `/api/v1/documents/${segment(id)}${suffix}`;
  }
  private access(options: DocumentAccessOptions): RequestOptions {
    return {
      ...options,
      headers:
        options.shareToken === undefined
          ? {}
          : { 'X-CMA-Document-Share-Token': options.shareToken },
    };
  }
  private get<T>(id: string, suffix: string, parse: Parser<T>, options: DocumentAccessOptions) {
    return read(this.config, this.path(id, suffix), parse, this.access(options));
  }
  private paged<T>(id: string, suffix: string, parse: Parser<T>, options: DocumentPageOptions) {
    return read(
      this.config,
      query(this.path(id, suffix), { limit: options.limit, cursor: options.cursor }),
      page(parse),
      this.access(options),
    );
  }
  private write<T>(
    id: string,
    suffix: string,
    method: string,
    body: unknown,
    parse: Parser<T>,
    options: DocumentCommandOptions,
  ) {
    const keyed = command(options);
    const headers = new Headers(keyed.headers);
    if (options.shareToken !== undefined)
      headers.set('X-CMA-Document-Share-Token', options.shareToken);
    return read(this.config, this.path(id, suffix), parse, {
      ...keyed,
      method,
      body,
      headers,
      expectedStatus: method === 'POST' ? 201 : 200,
    });
  }
}
