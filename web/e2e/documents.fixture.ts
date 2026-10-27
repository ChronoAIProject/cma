import type { Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
export const documentId = `doc_${'a'.repeat(32)}`;
export const initialMarkdown = `# Product brief\n\nA **clear** document with *context* and [a source](https://example.test).\n\n- [x] Research complete\n- [ ] Review findings\n\n| Milestone | Owner |\n| --- | --- |\n| Release | CMA |\n\n[font family="serif"]A thoughtful serif note.[/font] [date value="2026-10-22"]\n\n:::details {open=true}\n\n:::detailsSummary\n\nMore information\n\n:::\n\n:::detailsContent\n\nUseful hidden context.\n\n:::\n\n:::\n\n\`\`\`mermaid\nflowchart LR\n A[Research] --> B[Share]\n\`\`\`\n\nReady for edits.\n`;
export class LocalDocuments {
  readonly boundary = new LocalStartup();
  markdown = initialMarkdown;
  document = {
    id: documentId,
    owner_subject: 'local-owner',
    title: 'Product brief',
    revision: 3,
    source_revision: 1,
    sha256: 'a'.repeat(64),
    size_bytes: 100,
    created_at: '2026-10-22T00:00:00Z',
    updated_at: '2026-10-22T00:00:00Z',
    origin_agent_id: null,
    access: 'owner',
  };
  writes: {
    method: string;
    path: string;
    key: string | undefined;
    body: Record<string, unknown>;
  }[] = [];
  reads: { path: string; query: string; token: string | undefined }[] = [];
  comments: {
    id: string;
    author_subject: string;
    body: string;
    document_revision: number;
    created_at: string;
  }[] = [];
  shares: { subject: string; permission: string }[] = [];
  links: { id: string; permission: string; created_at: string }[] = [];
  exists = true;
  async route(page: Page, baseURL: string) {
    await this.boundary.route(page, baseURL);
    await page.route('**/api/v1/documents**', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const method = request.method();
      const body = method === 'GET' ? {} : (request.postDataJSON() as Record<string, unknown>);
      const reply = (json: unknown, status = 200) => route.fulfill({ status, json });
      if (method !== 'GET')
        this.writes.push({
          method,
          path: url.pathname,
          body,
          key: request.headers()['idempotency-key'],
        });
      else
        this.reads.push({
          path: url.pathname,
          query: url.search,
          token: request.headers()['x-cma-document-share-token'],
        });
      if (url.pathname === '/api/v1/documents') {
        if (method === 'POST') {
          this.exists = true;
          this.markdown = String(body.markdown);
          this.document.title = String(body.title);
          return reply(this.document);
        }
        return reply({ items: this.exists ? [this.document] : [], next: null });
      }
      if (url.pathname.endsWith('/source'))
        return reply({
          id: documentId,
          revision: this.document.source_revision,
          markdown: this.markdown,
        });
      if (url.pathname.endsWith('/comments')) {
        if (method === 'POST') {
          const comment = {
            id: 'comment_1',
            author_subject: 'local-owner',
            body: String(body.body),
            document_revision: this.document.source_revision,
            created_at: this.document.created_at,
          };
          this.comments.push(comment);
          this.document.revision++;
          return reply({ document: this.document, comment });
        }
        return reply({ items: this.comments, next: null });
      }
      if (url.pathname.endsWith('/shares')) return reply(this.shares);
      if (url.pathname.includes('/shares/')) {
        this.shares.push({
          subject: decodeURIComponent(url.pathname.split('/').pop()!),
          permission: String(body.permission),
        });
        this.document.revision++;
        return reply(this.document);
      }
      if (url.pathname.endsWith('/share-links')) {
        if (method === 'POST') {
          const link = {
            id: 'link_1',
            permission: String(body.permission),
            created_at: this.document.created_at,
          };
          this.links.push(link);
          this.document.revision++;
          return reply({ document: this.document, link });
        }
        return reply(this.links);
      }
      if (url.pathname === `/api/v1/documents/${documentId}`) {
        if (method === 'PATCH') {
          if (body.expected_revision !== this.document.revision)
            return reply({ detail: 'Revision conflict' }, 409);
          this.document.revision++;
          this.document.source_revision = this.document.revision;
          this.document.title = String(body.title);
          this.markdown = String(body.markdown);
        }
        return reply(this.document);
      }
      return reply({ detail: 'Unexpected local document route' }, 404);
    });
  }
}
