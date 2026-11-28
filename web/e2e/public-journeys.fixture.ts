import { createHash } from 'node:crypto';
import type { Page } from '@playwright/test';
import type { TriggerDocument, TriggerRevision } from '@cma/client';
import type { Harness } from './harness';
import type { PublisherRole } from './environment';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { materializeCurrentTriggerContent } from './public-trigger-fixture';
import { publicApiPath } from './public-api-path';

export async function publisherFixture(page: Page, baseURL: string, role: PublisherRole) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  const subject = `publisher-${role}`;
  const commands: Harness['commands'] = [];
  const resources: Harness['journal']['resources'] = [];
  const repository = { url: 'https://github.com/example/repository', ref: 'b'.repeat(40) };
  const materialize = (value: unknown) => materializeCurrentTriggerContent(value, repository, role);
  let document: TriggerDocument | undefined;
  let revision: TriggerRevision | undefined;
  const read = (path: string): unknown => {
    if (path === '/auth/session')
      return {
        authenticated: true,
        visitor: false,
        subject,
        capabilities: ['publish_triggers', 'create_sandbox', 'use_own_sandboxes'],
      };
    if (path === '/api/v1/triggers') return { items: document ? [document] : [] };
    if (path.endsWith('/revisions')) return [revision];
    if (path.endsWith('/stats'))
      return { totals: { launches_total: 0, agents_ready_total: 0 }, placements: [] };
    if (path.endsWith('/embed'))
      return {
        public_url: `${baseURL}/t/public-${role}/placement-${role}`,
        badge_url: 'https://embed.example.test/badge',
        label_text: 'Open in CMA',
        javascript_sri: 'sha384-synthetic',
        content_security_policy: {
          img_src: 'https://embed.example.test',
          script_src: 'https://embed.example.test',
        },
        snippets: {
          markdown: 'synthetic-markdown',
          html: 'synthetic-html',
          javascript: 'synthetic-script',
          react: 'synthetic-react',
          url: 'synthetic-url',
        },
      };
    if (path === `/api/v1/triggers/trigger-${role}`) return document;
    throw new Error(`Unexpected synthetic publisher read: ${path}`);
  };
  await page.route('**/*', async (route) => {
    const request = route.request();
    const path = publicApiPath(new URL(request.url()).pathname);
    if (request.url().startsWith('https://embed.example.test/badge?'))
      return route.fulfill({
        contentType: 'image/svg+xml',
        body: '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>',
      });
    if (path !== '/auth/session' && !path.startsWith('/api/v1/triggers')) return route.fallback();
    if (request.method() === 'GET') return route.fulfill({ json: read(path) });
    const body = request.postDataJSON();
    commands.push({
      method: request.method(),
      path,
      key: request.headers()['idempotency-key'],
      bodyHash: createHash('sha256')
        .update(request.postData() ?? '')
        .digest('hex'),
    });
    if (path === '/api/v1/triggers' && request.method() === 'POST') {
      document = {
        id: `trigger-${role}`,
        draft: materialize(body.content),
        draft_version: 1,
        availability: 'draft',
        revision_count: 0,
        placements: [],
        stats: { launches_total: 0, agents_ready_total: 0 },
        created_at: localTime,
        updated_at: localTime,
      };
      resources.push({ kind: 'triggers', id: document.id, subject });
      return route.fulfill({ status: 201, json: document });
    }
    if (path === `/api/v1/triggers/trigger-${role}` && request.method() === 'PUT') {
      document = {
        ...document!,
        draft: materialize(body.content),
        draft_version: document!.draft_version + 1,
      };
      return route.fulfill({ json: document });
    }
    if (path.endsWith('/placements')) {
      const placement = { id: `placement-${role}`, source_url: body.source_url, label: body.label };
      document = { ...document!, placements: [...document!.placements, placement] };
      return route.fulfill({ status: 201, json: placement });
    }
    if (path.endsWith('/publish')) {
      revision = {
        revision: 1,
        content: document!.draft,
        digest: 'c'.repeat(64),
        published_at: localTime,
      };
      document = {
        ...document!,
        availability: 'published',
        revision_count: 1,
        public_id: `public-${role}`,
      };
      return route.fulfill({
        status: 201,
        json: {
          ...revision,
          trigger_id: document.id,
          public_id: document.public_id,
          availability: 'published',
        },
      });
    }
    throw new Error(`Unexpected synthetic publisher write: ${path}`);
  });
  const harness = {
    config: { baseURL },
    fixture: {
      accounts: { [role]: { subject } },
      publicJourneys: {
        repository: { url: 'https://github.com/example/repository', reference: 'b'.repeat(40) },
        owners: {
          [role]: { model: 'fixture-model', embedUrl: `https://embed.example.test/${role}` },
        },
      },
      baselineServices: ['model'],
    },
    commands,
    journal: { resources },
    flush: async () => {},
    get: async (_page: Page, path: string) => read(path.split('?')[0]!),
  } as unknown as Harness;
  return { harness, boundary };
}
