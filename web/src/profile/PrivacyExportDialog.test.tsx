import { createHash } from 'node:crypto';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { PrivacyExportDialog } from './PrivacyExportDialog';

const owner = 'subject-a';
const agent = 'agt_one';
const payload = '{"content":"recorded"}\n';
const firstWindow = { turn: null, after: null, through: null, limit: 32 };
const laterWindow = { turn: 3, after: 4, through: 6, limit: 32 };
const inventoryPath = '/api/v1/account/privacy-export';
const continuationPath = `${inventoryPath}?cursor=agt_20`;

function inventory(agents: { id: string; title: string }[], next: string | null) {
  return Response.json({ owner_subject: owner, data_class: 'retained_conversation', agents, next });
}

function secondInventoryPage() {
  return inventory([{ id: 'agt_21', title: 'Agent 21' }], null);
}

function mockInventoryRetry() {
  const firstPage = Array.from({ length: 20 }, (_, i) => ({
    id: `agt_${String(i + 1).padStart(2, '0')}`,
    title: `Agent ${i + 1}`,
  }));
  const fetchInventory = vi
    .fn<(input: string, init?: RequestInit) => Promise<Response>>()
    .mockResolvedValueOnce(inventory(firstPage, 'agt_20'))
    .mockResolvedValueOnce(new Response(null, { status: 503 }))
    .mockRejectedValue(new Error('Unexpected export request'));
  vi.stubGlobal('fetch', fetchInventory);
  return fetchInventory;
}

function transcript(window: typeof firstWindow | typeof laterWindow) {
  return Response.json({
    owner_subject: owner,
    payload,
    manifest: {
      format: 'cma.retained-conversation-page',
      version: 1,
      agent_id: agent,
      through_seq: 6,
      window,
      records: 1,
      size_bytes: new TextEncoder().encode(payload).byteLength,
      sha256: createHash('sha256').update(payload).digest('hex'),
      coverage: {
        raw_history: false,
        completeness: 'not_certified',
        retention_window: 'not_reported_by_source',
        gaps: 'known_output_gaps_are_marked',
        includes: ['recorded_user_messages'],
        excludes: ['raw_runtime_records'],
      },
      next: null,
    },
  });
}

function mockExport(fetchTranscript: (init?: RequestInit) => Promise<Response>) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: string, init?: RequestInit) => {
      return input.endsWith('/transcript?limit=32')
        ? fetchTranscript(init)
        : inventory([{ id: agent, title: 'Example Agent' }], null);
    }),
  );
  return vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it('retries the failed continuation explicitly and makes the next page selectable', async () => {
  const fetchInventory = mockInventoryRetry().mockResolvedValueOnce(secondInventoryPage());
  const clickDownload = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  const actor = userEvent.setup();
  render(<PrivacyExportDialog owner={owner} onClose={vi.fn()} />);
  await screen.findByRole('option', { name: 'Agent 20' });
  await actor.selectOptions(screen.getByRole('combobox', { name: 'Agent' }), 'agt_20');

  await actor.click(screen.getByRole('button', { name: 'More Agents' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Select More Agents to try again.');
  expect(screen.getAllByRole('option')).toHaveLength(20);
  expect(screen.getByRole('combobox', { name: 'Agent' })).toHaveValue('agt_20');
  expect(fetchInventory).toHaveBeenCalledTimes(2);

  await actor.click(screen.getByRole('button', { name: 'More Agents' }));
  await screen.findByRole('option', { name: 'Agent 21' });
  expect(screen.getByRole('combobox', { name: 'Agent' })).toHaveValue('agt_21');
  expect(screen.getByRole('button', { name: 'Download conversation' })).toBeEnabled();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(fetchInventory.mock.calls.map(([url, init]) => [url, init?.method])).toEqual([
    [inventoryPath, 'GET'],
    [continuationPath, 'GET'],
    [continuationPath, 'GET'],
  ]);
  expect(clickDownload).not.toHaveBeenCalled();
});

it('aborts a continuation retry when the dialog closes and ignores its late response', async () => {
  let resolveResponse!: (response: Response) => void;
  const response = new Promise<Response>((resolve) => {
    resolveResponse = resolve;
  });
  const fetchInventory = mockInventoryRetry().mockReturnValueOnce(response);
  const clickDownload = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  const actor = userEvent.setup();
  const view = render(<PrivacyExportDialog owner={owner} onClose={() => view.unmount()} />);
  await screen.findByRole('option', { name: 'Agent 20' });
  await actor.click(screen.getByRole('button', { name: 'More Agents' }));
  await screen.findByRole('alert');
  await actor.click(screen.getByRole('button', { name: 'More Agents' }));
  await waitFor(() => expect(fetchInventory).toHaveBeenCalledTimes(3));
  const [url, init] = fetchInventory.mock.calls[2]!;
  expect(url).toBe(continuationPath);
  expect(init?.signal?.aborted).toBe(false);
  expect(screen.getByRole('button', { name: 'Download conversation' })).toBeDisabled();

  await actor.click(screen.getAllByRole('button', { name: /^Close$/ }).at(-1)!);
  expect(init?.signal?.aborted).toBe(true);
  await act(async () => {
    resolveResponse(secondInventoryPage());
    await response;
  });
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.queryByRole('option', { name: 'Agent 21' })).toBeNull();
  expect(fetchInventory).toHaveBeenCalledTimes(3);
  expect(clickDownload).not.toHaveBeenCalled();
});

it('withholds the file when a valid later page arrives as the first response', async () => {
  const clickDownload = mockExport(async () => transcript(laterWindow));
  const actor = userEvent.setup();
  render(<PrivacyExportDialog owner={owner} onClose={vi.fn()} />);
  await screen.findByRole('option', { name: 'Example Agent' });
  await actor.click(screen.getByRole('button', { name: 'Download conversation' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(/could not be verified/);
  expect(clickDownload).not.toHaveBeenCalled();
});

it('surfaces the server refusal code and lets the owner retry the same Agent', async () => {
  const clickDownload = mockExport(async () =>
    Response.json(
      {
        status: 503,
        code: 'privacy_export_source_unavailable',
        detail: 'The source is unavailable.',
      },
      { status: 503 },
    ),
  );
  const actor = userEvent.setup();
  render(<PrivacyExportDialog owner={owner} onClose={vi.fn()} />);
  await screen.findByRole('option', { name: 'Example Agent' });
  const button = screen.getByRole('button', { name: 'Download conversation' });
  await actor.click(button);
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Try again later (HTTP 503, privacy_export_source_unavailable).',
  );
  expect(button).toBeEnabled();
  expect(screen.getByRole('combobox', { name: 'Agent' })).toHaveValue(agent);
  expect(clickDownload).not.toHaveBeenCalled();
});

it('cancels an in-flight page and withholds the file after the dialog closes', async () => {
  let resolveResponse!: (response: Response) => void;
  let signal: AbortSignal | null | undefined;
  const response = new Promise<Response>((resolve) => {
    resolveResponse = resolve;
  });
  const fetchTranscript = vi.fn((init?: RequestInit) => {
    signal = init?.signal;
    return response;
  });
  const clickDownload = mockExport(fetchTranscript);
  const actor = userEvent.setup();
  const view = render(<PrivacyExportDialog owner={owner} onClose={() => view.unmount()} />);
  await screen.findByRole('option', { name: 'Example Agent' });
  await actor.click(screen.getByRole('button', { name: 'Download conversation' }));
  await waitFor(() => expect(fetchTranscript).toHaveBeenCalledOnce());
  await actor.click(screen.getAllByRole('button', { name: /^Close$/ }).at(-1)!);
  expect(signal?.aborted).toBe(true);
  await act(async () => {
    resolveResponse(transcript(firstWindow));
    await response;
  });
  expect(clickDownload).not.toHaveBeenCalled();
});
