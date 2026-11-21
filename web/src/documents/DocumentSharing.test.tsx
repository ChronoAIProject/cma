import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { DocumentError, type DocumentsClient, type DocumentView } from '../api/documents';
import { DocumentSharing } from './DocumentSharing';

const document: DocumentView = {
  id: 'doc_b93e52fee829494ba884eab90bb3a4d6',
  title: 'Project notes',
  owner_subject: 'owner',
  revision: 3,
  source_revision: 1,
  sha256: 'a'.repeat(64),
  size_bytes: 3,
  created_at: '2026-10-22T00:00:00Z',
  updated_at: '2026-10-22T00:00:00Z',
  origin_agent_id: null,
  access: 'owner',
};
const recipient = { subject: 'Person-A', display_name: 'Example Person' };

function setup() {
  const grant = vi.fn<DocumentsClient['grant']>().mockResolvedValue({ ...document, revision: 4 });
  const search = vi.fn<DocumentsClient['shareRecipients']>().mockResolvedValue([recipient]);
  const client = {
    grant,
    shareRecipients: search,
    shares: vi.fn().mockResolvedValue([]),
    links: vi.fn().mockResolvedValue([]),
  } as unknown as DocumentsClient;
  const onUpdated = vi.fn();
  render(
    <DocumentSharing
      client={client}
      document={document}
      open
      onUpdated={onUpdated}
      onClose={vi.fn()}
    />,
  );
  const input = screen.getByRole('textbox', { name: 'Find share recipients' });
  const submit = screen.getByRole('button', { name: 'Add person' });
  return { grant, search, input, submit, onUpdated };
}

async function choose(input: HTMLElement) {
  fireEvent.change(input, { target: { value: 'example' } });
  fireEvent.click(await screen.findByRole('button', { name: 'Select Example Person (Person-A)' }));
}

describe('Document share recipient selection', () => {
  it.each([1, 255])('can select an exact %i-character subject without a name', async (length) => {
    const { input, submit, grant, search } = setup();
    const subject = 'Q'.repeat(length);
    search.mockResolvedValue([{ subject, display_name: null }]);
    fireEvent.change(input, { target: { value: subject } });
    expect(submit).toBeDisabled();
    const candidate = await screen.findByRole('button', {
      name: `Select Name unavailable (${subject})`,
    });
    expect(search).toHaveBeenCalledWith(document.id, subject, { signal: expect.any(AbortSignal) });
    fireEvent.click(candidate);
    const selected = screen.getByRole('list', { name: 'Selected recipients' });
    expect(within(selected).getByText(subject)).toBeVisible();
    fireEvent.click(submit);
    expect(await screen.findByText('Sharing updated.')).toBeVisible();
    expect(grant).toHaveBeenCalledWith(document.id, subject, expect.anything(), expect.anything());
  });

  it('requires a resolved choice and invalidates it when the owner edits the search', async () => {
    const { input, submit, grant, search } = setup();
    expect(submit).toBeDisabled();
    await choose(input);
    expect(search).toHaveBeenCalledWith(document.id, 'example', {
      signal: expect.any(AbortSignal),
    });
    const selected = screen.getByRole('list', { name: 'Selected recipients' });
    expect(within(selected).getByText('Example Person')).toBeVisible();
    expect(within(selected).getByText('Person-A')).toBeVisible();
    expect(submit).toBeEnabled();
    fireEvent.change(input, { target: { value: 'person-a' } });
    expect(submit).toBeDisabled();
    expect(grant).not.toHaveBeenCalled();
  });

  it('keeps the selected identity after a refusal and reports why it cannot be shared', async () => {
    const { input, submit, grant, onUpdated } = setup();
    grant.mockRejectedValueOnce(
      new DocumentError('Unavailable', 409, 'share_recipient_unavailable'),
    );
    await choose(input);
    fireEvent.click(submit);
    expect(
      await screen.findByText('This recipient is unavailable. Search for an eligible CMA user.'),
    ).toBeVisible();
    expect(grant).toHaveBeenCalledWith(
      document.id,
      'Person-A',
      { expected_revision: 3, permission: 'read' },
      expect.objectContaining({ idempotencyKey: expect.any(String) }),
    );
    expect(screen.getByRole('list', { name: 'Selected recipients' })).toHaveTextContent('Person-A');
    expect(onUpdated).not.toHaveBeenCalled();
  });

  it('ignores superseded results and leaves grant disabled when search fails', async () => {
    const { input, submit, search } = setup();
    let resolveOld!: (results: readonly (typeof recipient)[]) => void;
    const oldSearch = new Promise<readonly (typeof recipient)[]>((resolve) => {
      resolveOld = resolve;
    });
    search.mockReturnValueOnce(oldSearch).mockRejectedValueOnce(new Error('Lookup is unavailable'));
    fireEvent.change(input, { target: { value: 'example' } });
    await waitFor(() => expect(search).toHaveBeenCalledTimes(1));
    fireEvent.change(input, { target: { value: 'current' } });
    expect(
      await screen.findByText('People could not be loaded. Adjust the search to retry.'),
    ).toBeVisible();
    await act(async () => resolveOld([recipient]));
    expect(search.mock.calls[0]?.[2]?.signal?.aborted).toBe(true);
    expect(screen.queryByRole('list', { name: 'Matching people' })).not.toBeInTheDocument();
    expect(submit).toBeDisabled();
  });

  it('retries an unavailable lookup with the original identity, revision and key', async () => {
    const { input, submit, grant, onUpdated } = setup();
    grant.mockRejectedValueOnce(
      new DocumentError(
        'Recipient lookup is temporarily unavailable. Try again.',
        503,
        'recipient_directory_unavailable',
      ),
    );
    await choose(input);
    fireEvent.click(submit);
    fireEvent.click(await screen.findByRole('button', { name: 'Retry change' }));
    expect(await screen.findByText('Sharing updated.')).toBeVisible();
    expect(grant).toHaveBeenCalledTimes(2);
    expect(grant.mock.calls[1]?.slice(0, 3)).toEqual(grant.mock.calls[0]?.slice(0, 3));
    expect(grant.mock.calls[1]?.[3].idempotencyKey).toBe(grant.mock.calls[0]?.[3].idempotencyKey);
    expect(onUpdated).toHaveBeenCalledWith({ ...document, revision: 4 });
  });
});
