import type { CommandOptions, ReadOptions } from './client';

export type DocumentPermission = 'read' | 'comment';
export interface DocumentView {
  readonly id: string;
  readonly owner_subject: string;
  readonly title: string;
  readonly revision: number;
  readonly source_revision: number;
  readonly sha256: string;
  readonly size_bytes: number;
  readonly created_at: string;
  readonly updated_at: string;
  readonly origin_agent_id: string | null;
  readonly access: 'owner' | DocumentPermission;
}
export interface DocumentSource {
  readonly id: string;
  readonly revision: number;
  readonly markdown: string;
}
export interface DocumentPage<T> {
  readonly items: readonly T[];
  readonly next: string | null;
}
export interface DocumentComment {
  readonly id: string;
  readonly author_subject: string;
  readonly body: string;
  readonly document_revision: number;
  readonly created_at: string;
}
export interface DocumentShare {
  readonly subject: string;
  readonly permission: DocumentPermission;
}
export interface DocumentLink {
  readonly id: string;
  readonly permission: DocumentPermission;
  readonly created_at: string;
}
export interface DocumentRevision {
  readonly revision: number;
  readonly title: string;
  readonly sha256: string;
  readonly size_bytes: number;
  readonly created_at: string;
  readonly author_subject: string;
}
export interface DocumentMutation {
  readonly document: DocumentView;
  readonly comment?: DocumentComment;
  readonly link?: DocumentLink;
}
export interface DocumentReadOptions extends ReadOptions {
  readonly shareToken?: string;
}
export interface DocumentPageOptions extends DocumentReadOptions {
  readonly cursor?: string;
  readonly limit?: number;
}
export interface DocumentsClient {
  list(
    options?: DocumentPageOptions & {
      readonly scope?: 'all' | 'owned' | 'shared';
      readonly q?: string;
    },
  ): Promise<DocumentPage<DocumentView>>;
  create(
    input: { readonly title: string; readonly markdown: string; readonly origin_agent_id?: string },
    options: CommandOptions,
  ): Promise<DocumentView>;
  read(id: string, options?: DocumentReadOptions): Promise<DocumentView>;
  source(
    id: string,
    options?: DocumentReadOptions & { readonly revision?: number },
  ): Promise<DocumentSource>;
  update(
    id: string,
    input: {
      readonly expected_revision: number;
      readonly title?: string;
      readonly markdown?: string;
    },
    options: CommandOptions,
  ): Promise<DocumentView>;
  revisions(id: string, options?: DocumentPageOptions): Promise<DocumentPage<DocumentRevision>>;
  shares(id: string, options?: ReadOptions): Promise<readonly DocumentShare[]>;
  grant(
    id: string,
    subject: string,
    input: { readonly expected_revision: number; readonly permission: DocumentPermission },
    options: CommandOptions,
  ): Promise<DocumentView>;
  revoke(
    id: string,
    subject: string,
    expectedRevision: number,
    options: CommandOptions,
  ): Promise<DocumentView>;
  links(id: string, options?: ReadOptions): Promise<readonly DocumentLink[]>;
  createLink(
    id: string,
    input: {
      readonly expected_revision: number;
      readonly permission: DocumentPermission;
      readonly token: string;
    },
    options: CommandOptions,
  ): Promise<DocumentMutation>;
  revokeLink(
    id: string,
    linkId: string,
    expectedRevision: number,
    options: CommandOptions,
  ): Promise<DocumentView>;
  comments(id: string, options?: DocumentPageOptions): Promise<DocumentPage<DocumentComment>>;
  comment(
    id: string,
    input: { readonly body: string; readonly document_revision?: number },
    options: CommandOptions & DocumentReadOptions,
  ): Promise<DocumentMutation>;
  removeComment(
    id: string,
    commentId: string,
    expectedRevision: number,
    options: CommandOptions & DocumentReadOptions,
  ): Promise<DocumentView>;
}

export class DocumentError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code?: string,
  ) {
    super(message);
    this.name = 'DocumentError';
  }
}

/** Keep link authority in the URL fragment, never in an HTTP query or referrer. */
export function newDocumentShareToken(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(32)), (value) =>
    value.toString(16).padStart(2, '0'),
  ).join('');
}
