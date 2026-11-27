import type { Dispatch, DispatchContext } from './context.js';
const access = (c: DispatchContext) => ({
  shareToken: c.fixture.request.headers['x-cma-document-share-token'],
});
const options = (c: DispatchContext) => ({ ...(c.options as Record<string, never>), ...access(c) });
const keyed = (c: DispatchContext) => ({ ...c.command, ...access(c) });
export const documentDispatch: Record<string, Dispatch> = {
  'GET /api/v1/documents': (c) => c.client.documents.list(c.options),
  'POST /api/v1/documents': (c) => c.client.documents.create(c.body, c.command),
  'GET /api/v1/documents/{id}': (c) => c.client.documents.read(c.param('id'), access(c)),
  'PATCH /api/v1/documents/{id}': (c) =>
    c.client.documents.update(c.param('id'), c.body, c.command),
  'DELETE /api/v1/documents/{id}': (c) =>
    c.client.documents.delete(c.param('id'), c.body, c.command),
  'GET /api/v1/documents/{id}/source': (c) =>
    c.client.documents.source(
      c.param('id'),
      c.query.has('revision') ? Number(c.query.get('revision')) : undefined,
      access(c),
    ),
  'GET /api/v1/documents/{id}/revisions': (c) =>
    c.client.documents.revisions(c.param('id'), options(c)),
  'GET /api/v1/documents/{id}/shares': (c) => c.client.documents.shares(c.param('id')),
  'GET /api/v1/documents/{id}/share-recipients': (c) =>
    c.client.documents.shareRecipients(c.param('id'), c.query.get('q') ?? undefined),
  'PUT /api/v1/documents/{id}/shares/{subject}': (c) =>
    c.client.documents.grant(c.param('id'), c.param('subject'), c.body, c.command),
  'DELETE /api/v1/documents/{id}/shares/{subject}': (c) =>
    c.client.documents.revoke(c.param('id'), c.param('subject'), c.body, c.command),
  'GET /api/v1/documents/{id}/share-links': (c) => c.client.documents.links(c.param('id')),
  'POST /api/v1/documents/{id}/share-links': (c) =>
    c.client.documents.createLink(c.param('id'), c.body, c.command),
  'DELETE /api/v1/documents/{id}/share-links/{link_id}': (c) =>
    c.client.documents.revokeLink(c.param('id'), c.param('link_id'), c.body, c.command),
  'GET /api/v1/documents/{id}/comments': (c) =>
    c.client.documents.comments(c.param('id'), options(c)),
  'POST /api/v1/documents/{id}/comments': (c) =>
    c.client.documents.comment(c.param('id'), c.body, keyed(c)),
  'DELETE /api/v1/documents/{id}/comments/{comment_id}': (c) =>
    c.client.documents.removeComment(c.param('id'), c.param('comment_id'), c.body, keyed(c)),
};
