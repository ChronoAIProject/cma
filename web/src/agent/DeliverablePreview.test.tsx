import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { createClient, type AnswerDeliverable } from '@cma/client';
import { DeliverablePreviewFrame } from './DeliverablePreviewFrame';
import { MessageStream } from './MessageStream';
import type { AnswerDeliveryAccess } from './answerDeliveries';

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

async function delivery(
  name: string,
  body: string | Uint8Array<ArrayBuffer>,
  index = 1,
): Promise<AnswerDeliverable> {
  const bytes = typeof body === 'string' ? new TextEncoder().encode(body) : body;
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  return {
    id: `delivery_${String(index).repeat(64)}`,
    reference: {
      agent_id: 'agt_1',
      resource: { kind: 'live_file', path: name },
      name,
      media_type: 'application/octet-stream',
      size_bytes: bytes.length,
      sha256: Array.from(digest, (byte) => byte.toString(16).padStart(2, '0')).join(''),
    },
    status: 'unchecked',
  };
}

function harness(
  items: readonly AnswerDeliverable[],
  download: (item: AnswerDeliverable, signal?: AbortSignal | null) => Promise<Response>,
  read = (item: AnswerDeliverable) => item,
) {
  const requests: URL[] = [];
  const sdk = createClient({
    baseUrl: 'https://cma.test',
    fetch: async (input, options) => {
      const url = new URL(String(input));
      requests.push(url);
      const path = url.pathname;
      const identity = { agent_id: 'agt_1', answer_id: 'event:3', turn_sequence: 1 };
      if (path.endsWith('/deliverables')) return Response.json({ ...identity, items });
      const item = items.find((entry) => path.includes(entry.id))!;
      if (path.endsWith('/download')) return download(item, options?.signal);
      return Response.json({ ...identity, item: read(item) });
    },
  });
  const access: AnswerDeliveryAccess = { agentId: 'agt_1', viewerId: 'reader', client: sdk.agents };
  const content = items
    .map(
      (item) =>
        `<!--cma-deliverable:v1:${btoa(JSON.stringify(item.reference)).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '')}-->`,
    )
    .join('\n');
  const view = (current: AnswerDeliveryAccess | null = access) => (
    <DeliverablePreviewFrame
      previewScope="reader:agt_1"
      onPreviewOpen={() => {}}
      detailsOpen={false}
      sidebar={null}
      center={
        <MessageStream
          conversation={{
            messages: [
              { id: 'event:3', role: 'assistant', status: 'complete', content, finalAnswer: true },
            ],
            toolCalls: [],
            reasoning: [],
            approvals: [],
            plans: [],
            recovery: [],
            cursor: 3,
          }}
          onApproval={() => {}}
          deliverables={current ?? undefined}
        />
      }
    />
  );
  return { ...render(view()), view, access, requests };
}
const opaque = (body: string) =>
  new Response(body, { headers: { 'content-type': 'application/octet-stream' } });
const panel = () => within(screen.getByRole('complementary', { name: 'Deliverable preview' }));

it('uses the image viewer only after the live file bytes match the registered digest', async () => {
  const bytes = Uint8Array.from(
    atob(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII=',
    ),
    (character) => character.charCodeAt(0),
  );
  const item = await delivery('pixel.png', bytes);
  const incorrect = bytes.slice();
  incorrect[0] = 0;
  let correct = false;
  harness(
    [item],
    async () =>
      new Response(correct ? bytes : incorrect, {
        headers: { 'content-type': 'application/octet-stream' },
      }),
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Preview pixel.png' }));
  expect(await panel().findByText('Preview failed; retry.')).toBeVisible();
  expect(panel().queryByRole('group', { name: 'Image zoom controls' })).toBeNull();
  correct = true;
  fireEvent.click(panel().getByRole('button', { name: 'Retry preview' }));
  expect(await panel().findByRole('group', { name: 'Image zoom controls' })).toBeVisible();
});

it('renders Markdown without activating untrusted links, images, HTML or SVG', async () => {
  const body =
    '# Safe heading\n[Click](https://untrusted.invalid/run)\n![remote](https://untrusted.invalid/pixel.png)\n<script>alert(1)</script>\n<svg onload="alert(1)"></svg>';
  const item = await delivery('report.md', body);
  harness([item], async () => opaque(body));
  fireEvent.click(await screen.findByRole('button', { name: 'Preview report.md' }));
  expect(await panel().findByRole('heading', { name: 'Safe heading' })).toBeVisible();
  expect(panel().queryByRole('link')).toBeNull();
  expect(panel().queryByRole('img')).toBeNull();
  expect(
    screen
      .getByRole('complementary', { name: 'Deliverable preview' })
      .querySelector('script, svg, iframe, object, embed'),
  ).toBeNull();
});

it('clears restricted preview bytes when independent resource permission is revoked', async () => {
  const body = 'Private delivery';
  const item = await delivery('private.txt', body);
  let denied = false;
  harness(
    [item],
    async () => opaque(body),
    (current) => (denied ? { ...current, status: 'denied' } : current),
  );
  fireEvent.click(await screen.findByRole('button', { name: 'Preview private.txt' }));
  expect(await panel().findByRole('textbox', { name: 'Contents of private.txt' })).toHaveValue(
    body,
  );
  denied = true;
  fireEvent.focus(window);
  expect(await panel().findByText('Access unavailable')).toBeVisible();
  expect(panel().queryByRole('textbox')).toBeNull();
  expect(panel().queryByRole('button', { name: 'Save' })).toBeNull();
});

it('applies the existing 1 MiB preview bound before requesting any file body', async () => {
  const body = 'a'.repeat(1024 * 1024 + 1);
  const item = await delivery('huge.txt', body);
  const download = vi.fn(async () => opaque(body));
  const { requests } = harness([item], download);
  fireEvent.click(await screen.findByRole('button', { name: 'Preview huge.txt' }));
  expect(
    await panel().findByText('Preview exceeds the 1 MiB limit. Download to view this file.'),
  ).toBeVisible();
  expect(panel().getByRole('button', { name: 'Download huge.txt' })).toBeEnabled();
  expect(download).not.toHaveBeenCalled();
  const reads = requests.filter((url) => url.pathname.endsWith(item.id));
  expect(reads).toHaveLength(1);
  expect(reads[0]!.searchParams.get('verify')).not.toBe('true');
});

it('offers a download fallback for PDF without opening an active document or fetching a preview body', async () => {
  const body = '%PDF-1.7\nUntrusted embedded actions';
  const item = await delivery('report.pdf', body);
  const download = vi.fn(async () => opaque(body));
  harness([item], download);
  fireEvent.click(await screen.findByRole('button', { name: 'Preview report.pdf' }));
  expect(
    await panel().findByText('Preview is not supported; download to view this file.'),
  ).toBeVisible();
  expect(panel().getByRole('button', { name: 'Download report.pdf' })).toBeEnabled();
  expect(download).not.toHaveBeenCalled();
  expect(panel().queryByRole('textbox')).toBeNull();
});

it.each([
  ['missing', 'File deleted'],
  ['changed', 'File changed since this answer'],
  ['denied', 'Access unavailable'],
  ['not_ready', 'Not ready'],
  ['too_large', 'Exceeds the resource limit'],
  ['unavailable', 'Download unavailable; retry'],
] as const)(
  'shows %s independently without retaining content or allowing download',
  async (status, label) => {
    const item = { ...(await delivery('result.txt', 'abc')), status };
    const download = vi.fn(async () => opaque('abc'));
    harness([item], download);
    fireEvent.click(await screen.findByRole('button', { name: 'Preview result.txt' }));
    expect(await panel().findByText(label)).toBeVisible();
    expect(panel().getByRole('button', { name: 'Download result.txt' })).toBeDisabled();
    expect(panel().queryByRole('textbox')).toBeNull();
    expect(download).not.toHaveBeenCalled();
  },
);

it('cancels A when selecting B and ignores A bytes arriving afterward', async () => {
  const first = await delivery('first.txt', 'abc');
  const second = await delivery('second.txt', 'other', 2);
  let release: ((response: Response) => void) | undefined;
  let firstSignal: AbortSignal | null | undefined;
  harness([first, second], async (item, signal) => {
    if (item.id === first.id) {
      firstSignal = signal;
      return new Promise((resolve) => {
        release = resolve;
      });
    }
    return opaque('other');
  });
  fireEvent.click(await screen.findByRole('button', { name: 'Preview first.txt' }));
  expect(panel().getByText('Loading preview…')).toBeVisible();
  await waitFor(() => expect(release).toBeDefined());
  fireEvent.click(screen.getByRole('button', { name: 'Preview second.txt' }));
  expect(await panel().findByRole('textbox')).toHaveValue('other');
  expect(firstSignal?.aborted).toBe(true);
  await act(async () => {
    release!(opaque('abc'));
  });
  expect(panel().getByRole('textbox')).toHaveValue('other');
  expect(screen.getByRole('button', { name: 'Preview second.txt' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
});

it('removes preview content immediately when the conversation loses its access context', async () => {
  const item = await delivery('private.txt', 'private');
  const view = harness([item], async () => opaque('private'));
  fireEvent.click(await screen.findByRole('button', { name: 'Preview private.txt' }));
  expect(await panel().findByRole('textbox')).toHaveValue('private');
  view.rerender(view.view(null));
  expect(screen.queryByRole('complementary', { name: 'Deliverable preview' })).toBeNull();
  expect(screen.queryByDisplayValue('private')).toBeNull();
});

it('retries a failed body read independently and never renders mismatched bytes', async () => {
  const item = await delivery('result.txt', 'abc');
  let correct = false;
  harness([item], async () => opaque(correct ? 'abc' : 'bad'));
  fireEvent.click(await screen.findByRole('button', { name: 'Preview result.txt' }));
  expect(await panel().findByText('Preview failed; retry.')).toBeVisible();
  expect(panel().queryByRole('textbox')).toBeNull();
  correct = true;
  fireEvent.click(panel().getByRole('button', { name: 'Retry preview' }));
  expect(await panel().findByRole('textbox')).toHaveValue('abc');
});

it.each(['html', 'svg'])('keeps raw %s inert and read-only', async (extension) => {
  const body =
    '<script>alert(1)</script><svg onload="alert(2)"></svg><a href="/api/delete">run</a>';
  const item = await delivery(`unsafe.${extension}`, body);
  harness([item], async () => opaque(body));
  fireEvent.click(await screen.findByRole('button', { name: `Preview unsafe.${extension}` }));
  const text = await panel().findByRole('textbox');
  expect(text).toHaveValue(body);
  expect(text).toHaveAttribute('readonly');
  expect(panel().queryByRole('link')).toBeNull();
  expect(panel().queryByRole('button', { name: 'Save' })).toBeNull();
});

it('labels a newer document revision while showing only the referenced revision', async () => {
  const original = await delivery('report.md', '# Original revision');
  const item: AnswerDeliverable = {
    ...original,
    status: 'version_changed',
    reference: {
      ...original.reference,
      resource: { kind: 'document', id: `doc_${'c'.repeat(32)}`, revision: 1 },
      media_type: 'text/markdown',
    },
  };
  harness([item], async () => opaque('# Original revision'));
  fireEvent.click(await screen.findByRole('button', { name: 'Preview report.md' }));
  expect(await panel().findByRole('heading', { name: 'Original revision' })).toBeVisible();
  expect(
    panel().getByText('Document has a newer revision; download keeps this revision'),
  ).toBeVisible();
  expect(panel().getByText('Document revision 1')).toBeVisible();
});

it('reopens the same card with fresh state after a failed download', async () => {
  const item = await delivery('result.txt', 'abc');
  let downloads = 0;
  harness([item], async () => {
    if (++downloads === 2) throw new Error('Download interrupted');
    return opaque('abc');
  });
  const card = await screen.findByRole('button', { name: 'Preview result.txt' });
  fireEvent.click(card);
  expect(await panel().findByRole('textbox')).toHaveValue('abc');
  fireEvent.click(panel().getByRole('button', { name: 'Download result.txt' }));
  expect(await panel().findByText('Preview failed; retry.')).toBeVisible();
  fireEvent.click(card);
  expect(await panel().findByRole('textbox')).toHaveValue('abc');
});
