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
        : Response.json({
            owner_subject: owner,
            data_class: 'retained_conversation',
            agents: [{ id: agent, title: 'Example Agent' }],
            next: null,
          });
    }),
  );
  return vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
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
