import { ProtocolError } from '@chronoai/cma-sdk';
import { DocumentError } from '../documents';
import type {
  DocumentsClient,
  DocumentReadOptions,
  DocumentView,
  DocumentPage,
  DocumentMutation,
  DocumentUsage,
  DocumentUsageCounter,
} from '../documents';
import type { CommandOptions } from '../client';

function path(id?: string, suffix = ''): string {
  return `/api/v1/documents${id ? `/${encodeURIComponent(id)}` : ''}${suffix}`;
}
function query(values: Record<string, unknown>): string {
  const pairs = Object.entries(values)
    .filter(([, value]) => value !== undefined)
    .map(([key, value]) => [key, String(value)]);
  return pairs.length ? `?${new URLSearchParams(pairs)}` : '';
}
async function request(
  url: string,
  options?: DocumentReadOptions & Partial<CommandOptions>,
  method = 'GET',
  input?: unknown,
): Promise<unknown> {
  const headers: Record<string, string> = { Accept: 'application/json' };
  if (options?.shareToken) headers['X-CMA-Document-Share-Token'] = options.shareToken;
  if (options?.idempotencyKey) headers['Idempotency-Key'] = options.idempotencyKey;
  if (input !== undefined) headers['Content-Type'] = 'application/json';
  const response = await fetch(url, {
    method,
    headers,
    body: input === undefined ? undefined : JSON.stringify(input),
    signal: options?.signal,
    credentials: 'same-origin',
    redirect: 'error',
    cache: 'no-store',
    referrerPolicy: 'no-referrer',
  });
  const value: unknown = await response.json().catch(() => null);
  if (!response.ok) {
    const problem = value as {
      detail?: unknown;
      code?: unknown;
      limit?: unknown;
      live?: unknown;
    } | null;
    throw new DocumentError(
      typeof problem?.detail === 'string'
        ? problem.detail
        : 'This document could not be loaded. Try again.',
      response.status,
      typeof problem?.code === 'string' ? problem.code : undefined,
      {
        limit: counterValue(problem?.limit) ? problem.limit : undefined,
        live: counterValue(problem?.live) ? problem.live : undefined,
      },
    );
  }
  return value;
}
function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
function counterValue(value: unknown): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}
function counter(value: unknown): value is DocumentUsageCounter | null {
  return value === null || (record(value) && counterValue(value.used) && counterValue(value.limit));
}
function usage(value: unknown): DocumentUsage {
  if (
    !record(value) ||
    (value.enforcement !== 'enforced' && value.enforcement !== 'not_enforced') ||
    !counter(value.documents) ||
    !counter(value.bytes) ||
    !counter(value.objects) ||
    !(value.pending_deletions === null || counterValue(value.pending_deletions))
  )
    throw new ProtocolError('CMA returned invalid document usage.');
  return value as unknown as DocumentUsage;
}
function view(value: unknown): DocumentView {
  if (
    !record(value) ||
    typeof value.id !== 'string' ||
    !/^doc_[a-f0-9]{32}$/.test(value.id) ||
    typeof value.title !== 'string' ||
    typeof value.owner_subject !== 'string' ||
    !Number.isSafeInteger(value.revision) ||
    Number(value.revision) < 1 ||
    !Number.isSafeInteger(value.source_revision) ||
    Number(value.source_revision) < 1 ||
    !['owner', 'read', 'comment'].includes(String(value.access)) ||
    typeof value.sha256 !== 'string' ||
    !/^[a-f0-9]{64}$/.test(value.sha256) ||
    !Number.isSafeInteger(value.size_bytes) ||
    Number(value.size_bytes) < 1 ||
    Number(value.size_bytes) > 1024 * 1024 ||
    typeof value.updated_at !== 'string' ||
    typeof value.created_at !== 'string'
  )
    throw new ProtocolError('CMA returned invalid document metadata.');
  return value as unknown as DocumentView;
}
function page<T>(value: unknown, decode: (item: unknown) => T): DocumentPage<T> {
  if (
    !record(value) ||
    !Array.isArray(value.items) ||
    value.items.length > 50 ||
    !(value.next === null || typeof value.next === 'string')
  )
    throw new ProtocolError('CMA returned an invalid document page.');
  return { items: value.items.map(decode), next: value.next };
}
function mutation(value: unknown): DocumentMutation {
  if (!record(value)) throw new ProtocolError('CMA did not confirm this document update.');
  return { ...value, document: view(value.document) } as unknown as DocumentMutation;
}
export function documentsClient(): DocumentsClient {
  return {
    async documentUsage(options) {
      return usage(await request('/api/v1/usage/documents', options));
    },
    async deleteDocument(id, options) {
      const result = await request(path(id), options, 'DELETE', {
        expected_revision: options.expectedRevision,
      });
      if (
        !record(result) ||
        result.id !== id ||
        !Number.isSafeInteger(result.revision) ||
        Number(result.revision) < 1 ||
        result.revision !== options.expectedRevision + 1 ||
        typeof result.deleted_at !== 'string' ||
        !result.deleted_at ||
        (result.purge !== 'pending' && result.purge !== 'complete' && result.purge !== 'blocked')
      )
        throw new ProtocolError('CMA did not confirm this document deletion.');
      return result as unknown as import('../documents').DocumentDeletion;
    },
    async list(options) {
      return page(
        await request(
          path() +
            query({
              scope: options?.scope,
              q: options?.q,
              cursor: options?.cursor,
              limit: options?.limit,
            }),
          options,
        ),
        view,
      );
    },
    async create(input, options) {
      return view(await request(path(), options, 'POST', input));
    },
    async read(id, options) {
      const result = view(await request(path(id), options));
      if (result.id !== id) throw new ProtocolError('Document identity mismatch.');
      return result;
    },
    async source(id, options) {
      const result = await request(
        path(id, '/source') + query({ revision: options?.revision }),
        options,
      );
      if (
        !record(result) ||
        result.id !== id ||
        !Number.isSafeInteger(result.revision) ||
        (options?.revision !== undefined && result.revision !== options.revision) ||
        typeof result.markdown !== 'string' ||
        new TextEncoder().encode(result.markdown).length > 1024 * 1024
      )
        throw new ProtocolError('CMA returned an invalid document source.');
      return { id, revision: Number(result.revision), markdown: result.markdown };
    },
    async update(id, input, options) {
      return view(await request(path(id), options, 'PATCH', input));
    },
    async revisions(id, options) {
      return page(
        await request(
          path(id, '/revisions') + query({ cursor: options?.cursor, limit: options?.limit }),
          options,
        ),
        (item) => {
          if (
            !record(item) ||
            !Number.isSafeInteger(item.revision) ||
            typeof item.title !== 'string'
          )
            throw new ProtocolError('Invalid document revision.');
          return item as unknown as import('../documents').DocumentRevision;
        },
      );
    },
    async shares(id, options) {
      const result = await request(path(id, '/shares'), options);
      if (!Array.isArray(result)) throw new ProtocolError('Invalid document recipients.');
      return result;
    },
    async grant(id, subject, input, options) {
      return view(
        await request(path(id, `/shares/${encodeURIComponent(subject)}`), options, 'PUT', input),
      );
    },
    async revoke(id, subject, expectedRevision, options) {
      return view(
        await request(path(id, `/shares/${encodeURIComponent(subject)}`), options, 'DELETE', {
          expected_revision: expectedRevision,
        }),
      );
    },
    async links(id, options) {
      const result = await request(path(id, '/share-links'), options);
      if (!Array.isArray(result)) throw new ProtocolError('Invalid document links.');
      return result;
    },
    async createLink(id, input, options) {
      return mutation(await request(path(id, '/share-links'), options, 'POST', input));
    },
    async revokeLink(id, linkId, expectedRevision, options) {
      return view(
        await request(path(id, `/share-links/${encodeURIComponent(linkId)}`), options, 'DELETE', {
          expected_revision: expectedRevision,
        }),
      );
    },
    async comments(id, options) {
      return page(
        await request(
          path(id, '/comments') + query({ cursor: options?.cursor, limit: options?.limit }),
          options,
        ),
        (item) => {
          if (
            !record(item) ||
            typeof item.id !== 'string' ||
            typeof item.body !== 'string' ||
            typeof item.author_subject !== 'string'
          )
            throw new ProtocolError('Invalid document comment.');
          return item as unknown as import('../documents').DocumentComment;
        },
      );
    },
    async comment(id, input, options) {
      return mutation(await request(path(id, '/comments'), options, 'POST', input));
    },
    async removeComment(id, commentId, expectedRevision, options) {
      return view(
        await request(path(id, `/comments/${encodeURIComponent(commentId)}`), options, 'DELETE', {
          expected_revision: expectedRevision,
        }),
      );
    },
  };
}
