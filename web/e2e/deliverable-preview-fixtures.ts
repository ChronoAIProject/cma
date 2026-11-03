import { createHash } from 'node:crypto';
import type { Page } from '@playwright/test';
import type { AnswerDeliverable } from '../../sdk/typescript/src/index';
import { LocalStartup } from './agent-startup-fixtures';

export const evidence = '/Users/liuyitian/code/cma-ui-worktrees/evidence/ticket-08';
const digest = (body: Buffer) => createHash('sha256').update(body).digest('hex');
const unsafe =
  '<script>window.previewExecuted = true</script><svg onload="window.previewExecuted=true"></svg>';
export const samples = [
  ['result.txt', Buffer.from('Verified answer file')],
  [
    'report.md',
    Buffer.from(
      `# Safe report\n[untrusted](https://untrusted.invalid/run)\n![tracking](https://untrusted.invalid/pixel.png)\n${unsafe}`,
    ),
  ],
  ['unsafe.html', Buffer.from(`${unsafe}<a href="/api/v1/agents">same origin link</a>`)],
  [
    'unsafe.svg',
    Buffer.from(
      `<svg xmlns="http://www.w3.org/2000/svg" onload="window.previewExecuted=true"><script>window.previewExecuted=true</script></svg>`,
    ),
  ],
  ['report.pdf', Buffer.from('%PDF-1.7\nDownload fallback')],
  [
    'pixel.png',
    Buffer.from(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII=',
      'base64',
    ),
  ],
] as const;

export async function previewBoundary(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  const items: AnswerDeliverable[] = samples.map(([name, body], index) => ({
    id: `delivery_${String(index + 1).repeat(64)}`,
    reference: {
      agent_id: 'agt_contract',
      resource: { kind: 'live_file', path: name },
      name,
      media_type: 'application/octet-stream',
      size_bytes: body.length,
      sha256: digest(body),
    },
    status: 'unchecked',
  }));
  const marker = items
    .map(
      (item) =>
        `<!--cma-deliverable:v1:${Buffer.from(JSON.stringify(item.reference)).toString('base64url')}-->`,
    )
    .join('\n');
  boundary.events = Array.from({ length: 24 }, (_, index) => {
    const sequence = index * 4 + 1;
    return [
      { seq: sequence, kind: { type: 'turn_started', turn_sequence: sequence } },
      { seq: sequence + 1, kind: { type: 'owner_message', content: `Task ${index}` } },
      {
        seq: sequence + 2,
        kind: {
          type: 'agent_message',
          content:
            index === 12
              ? `Verified deliveries.\n${marker}`
              : `Answer ${index}. ${'Recorded conversation context. '.repeat(12)}`,
        },
      },
      { seq: sequence + 3, kind: { type: 'turn_completed', turn_sequence: sequence } },
    ].map((entry) => ({ ...entry, appended_at: '2026-11-03T01:00:00Z' }));
  }).flat();
  await boundary.route(page, baseURL);
  const control = {
    revoked: false,
    saveStarted: false,
    saved: false,
    releaseSave: () => {},
    requests: [] as string[],
  };
  page.once('close', () => control.releaseSave());
  await page.route('**/api/v1/agents/agt_contract/answers/**', async (route) => {
    const path = decodeURIComponent(new URL(route.request().url()).pathname);
    control.requests.push(path);
    const identity = { agent_id: 'agt_contract', answer_id: 'event:51', turn_sequence: 49 };
    if (path.endsWith('/deliverables')) return route.fulfill({ json: { ...identity, items } });
    const item = items.find((value) => path.includes(value.id))!;
    if (path.endsWith('/download'))
      return route.fulfill({
        contentType: 'application/octet-stream',
        body: samples.find(([name]) => name === item.reference.name)![1],
      });
    return route.fulfill({
      json: { ...identity, item: control.revoked ? { ...item, status: 'denied' } : item },
    });
  });
  let file = Buffer.from('Original notes');
  await page.route('**/api/v1/agents/agt_contract/files**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/tree'))
      return route.fulfill({
        json: {
          path: '',
          entries: [
            {
              path: 'notes.txt',
              name: 'notes.txt',
              kind: 'file',
              size_bytes: file.length,
              hidden: false,
            },
          ],
        },
      });
    if (url.pathname.endsWith('/content'))
      return route.fulfill({
        json: {
          path: 'notes.txt',
          content_base64: file.toString('base64'),
          size_bytes: file.length,
          sha256: digest(file),
        },
      });
    if (route.request().method() === 'POST') {
      control.saveStarted = true;
      const body = route.request().postDataJSON() as { content_base64: string };
      await new Promise<void>((resolve) => {
        control.releaseSave = resolve;
      });
      file = Buffer.from(body.content_base64, 'base64');
      await route.fulfill({
        json: { path: 'notes.txt', size_bytes: file.length, sha256: digest(file) },
      });
      control.saved = true;
      return;
    }
    throw new Error(`Unexpected file request ${url.pathname}`);
  });
  await page.route('**/api/v1/agents/agt_contract/attachments?*', async (route) => {
    const name = new URL(route.request().url()).searchParams.get('name')!;
    const body = route.request().postDataBuffer()!;
    const id = `att_${'a'.repeat(64)}`;
    await route.fulfill({
      status: 201,
      json: {
        id,
        agent_id: 'agt_contract',
        name,
        path: `/home/attachments/agt_contract/${id}/${name}`,
        size_bytes: body.length,
        media_type: route.request().headers()['content-type'],
        sha256: digest(body),
      },
    });
  });
  return { boundary, control };
}
