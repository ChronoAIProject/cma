import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { DocumentPage, DocumentsClient, DocumentView } from '../api/documents';
import { DocumentRegistry } from './DocumentRegistry';

function client(
  list = vi.fn<DocumentsClient['list']>().mockResolvedValue({ items: [], next: null }),
) {
  return {
    list,
    create: vi.fn(),
    read: vi.fn(),
    source: vi.fn(),
    update: vi.fn(),
    revisions: vi.fn(),
    shares: vi.fn(),
    shareRecipients: vi.fn(),
    grant: vi.fn(),
    revoke: vi.fn(),
    links: vi.fn(),
    createLink: vi.fn(),
    revokeLink: vi.fn(),
    comments: vi.fn(),
    comment: vi.fn(),
    removeComment: vi.fn(),
  } satisfies DocumentsClient;
}

const advance = async (milliseconds = 0) => {
  await act(() => vi.advanceTimersByTimeAsync(milliseconds));
};
const scopes = [
  { scope: 'all', label: 'All', heading: 'Your library' },
  { scope: 'owned', label: 'Yours', heading: 'Your library' },
  { scope: 'shared', label: 'Shared with you', heading: 'Shared documents' },
] as const;

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const whitespace = ['', ' ', '   ', '\u3000', '\u00a0', '\t', '\r\n', ' \t\u3000\u00a0 '];
const document = (letter: string, title: string): DocumentView => ({
  id: `doc_${letter.repeat(32)}`,
  owner_subject: 'owner',
  title,
  revision: 1,
  source_revision: 1,
  sha256: 'a'.repeat(64),
  size_bytes: 10,
  created_at: '2026-11-19T00:00:00Z',
  updated_at: '2026-11-19T00:00:00Z',
  origin_agent_id: null,
  access: 'owner',
});
const change = (value: string) =>
  fireEvent.change(screen.getByRole('searchbox'), { target: { value } });
const mount = (api: DocumentsClient) =>
  render(<DocumentRegistry client={api} viewer="owner" canCreate onNavigate={vi.fn()} />);

for (const populated of [false, true]) {
  it.each(scopes)(
    `keeps normal $scope results for whitespace (populated=${String(populated)})`,
    async ({ scope, label, heading }) => {
      const item = document('a', 'Retained document');
      const api = client(
        vi
          .fn<DocumentsClient['list']>()
          .mockResolvedValue({ items: populated ? [item] : [], next: null }),
      );
      mount(api);
      await advance();
      if (scope !== 'all') {
        fireEvent.click(screen.getByRole('button', { name: label }));
        await advance();
      }
      const calls = api.list.mock.calls.length;
      for (const value of whitespace) {
        change(value);
        await advance(200);
        expect(screen.getByRole('searchbox')).toHaveValue(
          value.replaceAll('\r', '').replaceAll('\n', ''),
        );
        expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent(heading);
        expect(screen.queryByText('No matching documents')).not.toBeInTheDocument();
        if (populated)
          expect(screen.getByRole('link', { name: /Retained document/ })).toBeVisible();
        else expect(screen.getByText('A place for your next idea')).toBeVisible();
        expect(api.list).toHaveBeenCalledTimes(calls);
        expect(api.list).toHaveBeenLastCalledWith(
          expect.objectContaining({ scope, q: undefined, cursor: undefined }),
        );
      }
      expect(api.create).not.toHaveBeenCalled();
      expect(api.update).not.toHaveBeenCalled();
    },
  );
}

it('debounces normalized search terms while preserving raw input and internal whitespace', async () => {
  const api = client();
  mount(api);
  await advance();
  change('  report  ');
  expect(screen.getByRole('searchbox')).toHaveValue('  report  ');
  expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Search results');
  await advance(199);
  expect(api.list).toHaveBeenCalledTimes(1);
  await advance(1);
  expect(api.list).toHaveBeenLastCalledWith(
    expect.objectContaining({ q: 'report', cursor: undefined }),
  );
  expect(screen.getByText('No matching documents')).toBeVisible();
  change('report  draft');
  await advance(200);
  expect(api.list).toHaveBeenLastCalledWith(expect.objectContaining({ q: 'report  draft' }));
});

it.each(['', 'report'])(
  'retains loaded pages and the next cursor when only surrounding whitespace changes: %j',
  async (query) => {
    const first = document('a', 'First document');
    const second = document('b', 'Second document');
    const third = document('c', 'Third document');
    const api = client(
      vi.fn<DocumentsClient['list']>().mockImplementation(async (options) => {
        if (options?.cursor === 'page-2') return { items: [second], next: 'page-3' };
        if (options?.cursor === 'page-3') return { items: [third], next: null };
        return { items: [first], next: 'page-2' };
      }),
    );
    mount(api);
    await advance();
    if (query) {
      change(query);
      await advance(200);
    }
    fireEvent.click(screen.getByRole('button', { name: 'Load more' }));
    await advance(200);
    const calls = api.list.mock.calls.length;
    change(` \u3000${query}\u00a0 `);
    fireEvent.click(screen.getByRole('button', { name: 'All' }));
    await advance(200);
    expect(api.list).toHaveBeenCalledTimes(calls);
    expect(screen.getByRole('link', { name: /First document/ })).toBeVisible();
    expect(screen.getByRole('link', { name: /Second document/ })).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Load more' }));
    await advance(200);
    expect(api.list).toHaveBeenLastCalledWith(
      expect.objectContaining({ q: query || undefined, cursor: 'page-3' }),
    );
    expect(screen.getByRole('link', { name: /Third document/ })).toBeVisible();
  },
);

it.each(['blank', 'query', 'scope'])(
  'discards the search cursor before the next %s list can be loaded',
  async (transition) => {
    const api = client(
      vi
        .fn<DocumentsClient['list']>()
        .mockResolvedValue({ items: [document('a', 'Previous result')], next: 'old-cursor' }),
    );
    mount(api);
    await advance();
    change('report');
    await advance(200);
    fireEvent.click(screen.getByRole('button', { name: 'Load more' }));
    await advance(200);
    if (transition === 'scope')
      fireEvent.click(screen.getByRole('button', { name: 'Shared with you' }));
    else change(transition === 'blank' ? ' \u3000 ' : 'draft');
    expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: /Previous result/ })).not.toBeInTheDocument();
    await advance(transition === 'blank' ? 0 : 200);
    expect(api.list).toHaveBeenLastCalledWith(
      expect.objectContaining({
        scope: transition === 'scope' ? 'shared' : 'all',
        q: transition === 'blank' ? undefined : transition === 'scope' ? 'report' : 'draft',
        cursor: undefined,
      }),
    );
  },
);

for (const rejection of [false, true]) {
  it.each(['blank', 'query', 'scope'])(
    `ignores an obsolete %s response (rejection=${String(rejection)})`,
    async (transition) => {
      let resolve!: (page: DocumentPage<DocumentView>) => void;
      let reject!: (error: Error) => void;
      const pending = new Promise<DocumentPage<DocumentView>>((yes, no) => {
        resolve = yes;
        reject = no;
      });
      const api = client();
      mount(api);
      await advance();
      api.list.mockReturnValueOnce(pending);
      change('old');
      await advance(200);
      const oldSignal = api.list.mock.lastCall?.[0]?.signal;
      api.list.mockResolvedValueOnce({ items: [document('b', 'Current result')], next: null });
      if (transition === 'scope')
        fireEvent.click(screen.getByRole('button', { name: 'Shared with you' }));
      else change(transition === 'blank' ? '   ' : 'current');
      expect(oldSignal?.aborted).toBe(true);
      await advance(200);
      await act(async () => {
        if (rejection) reject(new Error('Obsolete request failed'));
        else resolve({ items: [document('a', 'Obsolete result')], next: 'obsolete-cursor' });
      });
      expect(screen.getByRole('link', { name: /Current result/ })).toBeVisible();
      expect(screen.queryByRole('link', { name: /Obsolete result/ })).not.toBeInTheDocument();
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Load more' })).not.toBeInTheDocument();
    },
  );
}

it('keeps an empty-query failure and Retry visible through equivalent whitespace edits', async () => {
  const api = client();
  mount(api);
  await advance();
  change('report');
  await advance(200);
  api.list.mockRejectedValueOnce(new Error('Artifacts are unavailable'));
  change('   ');
  await advance();
  expect(screen.getByRole('alert')).toHaveTextContent('Artifacts are unavailable');
  expect(screen.queryByText('A place for your next idea')).not.toBeInTheDocument();
  expect(screen.queryByText('No matching documents')).not.toBeInTheDocument();
  const calls = api.list.mock.calls.length;
  change('\u3000\u00a0');
  await advance(200);
  expect(api.list).toHaveBeenCalledTimes(calls);
  expect(screen.getByRole('button', { name: 'Retry' })).toBeVisible();
  api.list.mockResolvedValueOnce({ items: [document('a', 'Recovered document')], next: null });
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await advance();
  expect(api.list).toHaveBeenLastCalledWith(
    expect.objectContaining({ scope: 'all', q: undefined, cursor: undefined }),
  );
  expect(screen.getByRole('link', { name: /Recovered document/ })).toBeVisible();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});
