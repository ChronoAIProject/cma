import { useRef } from 'react';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { Composer } from './Composer';
import type { ComposerSuggestionEditorHandle } from './ComposerSuggestionEditor';
import { QueuedInputOutcomes } from './QueuedInputOutcomes';
import { useAgentDraft } from './draft';
import { agentDraftKey, readAgentDraft } from './draftStorage';

beforeEach(() => sessionStorage.clear());

function setup() {
  const client = agentClient();
  const send = vi.fn();
  function Surface({ owner = 'alice', agent = 'agt_1', pending = false } = {}) {
    const draft = useAgentDraft(client, owner, agent);
    const editorRef = useRef<ComposerSuggestionEditorHandle>(null);
    return (
      <>
        <QueuedInputOutcomes
          items={[
            {
              commandId: 'cmd_expired',
              responseId: 'resp_expired',
              kind: 'expired',
              terminal: true,
              input: 'Original message\nwith a second line',
            },
          ]}
          onRefresh={vi.fn()}
          onRestore={(text) => editorRef.current?.restore(text, () => {})}
          restoreDisabled={draft.recoveryBlocked || pending}
        />
        <button
          onClick={() =>
            draft.setAttachments([
              {
                key: 'retained',
                status: 'reselect',
                description: { name: 'keep.txt', size: 6, mediaType: 'text/plain' },
              },
            ])
          }
        >
          Add existing attachment
        </button>
        <Composer
          key={`${owner}:${agent}`}
          editorRef={editorRef}
          draft={draft}
          status="ready"
          running={false}
          requestPending={pending}
          inputAvailability={{ allowed: true }}
          busy={false}
          onSend={send}
          onStop={vi.fn()}
        />
      </>
    );
  }
  const view = render(<Surface />);
  return { ...view, Surface, send, field: () => screen.getByRole('textbox', { name: 'Message' }) };
}

it('restores and focuses an empty composer, persists through reload, and waits for explicit Send', async () => {
  const { field, Surface, send, unmount } = setup();
  fireEvent.click(screen.getByRole('button', { name: 'Restore to draft' }));
  expect(field()).toHaveValue('Original message\nwith a second line');
  expect(field()).toHaveFocus();
  expect(send).not.toHaveBeenCalled();
  expect(readAgentDraft('alice', 'agt_1').content).toBe('Original message\nwith a second line');
  unmount();
  render(<Surface />);
  expect(field()).toHaveValue('Original message\nwith a second line');
  expect(send).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(send).toHaveBeenCalledExactlyOnceWith(
    'Original message\nwith a second line',
    expect.any(Function),
  );
});

it('defaults to Keep editing and appends only after a separate explicit choice', async () => {
  const user = userEvent.setup();
  const { field, send } = setup();
  fireEvent.change(field(), { target: { value: '  Keep my draft  ' } });
  await user.click(screen.getByRole('button', { name: 'Restore to draft' }));
  let dialog = screen.getByRole('dialog', { name: 'Keep your draft?' });
  expect(within(dialog).getByRole('button', { name: 'Keep editing' })).toHaveFocus();
  expect(within(dialog).queryByRole('button', { name: 'Replace text' })).not.toBeInTheDocument();
  await user.keyboard('{Enter}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(field()).toHaveValue('  Keep my draft  ');
  expect(field()).toHaveFocus();
  await user.click(screen.getByRole('button', { name: 'Restore to draft' }));
  dialog = screen.getByRole('dialog', { name: 'Keep your draft?' });
  await user.click(within(dialog).getByRole('button', { name: 'Add to current draft' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(field()).toHaveValue('  Keep my draft  \nOriginal message\nwith a second line');
  expect(field()).toHaveFocus();
  expect(send).not.toHaveBeenCalled();
});

it('asks before adding to an attachment-only draft and keeps its attachment', async () => {
  const { field, send } = setup();
  fireEvent.click(screen.getByRole('button', { name: 'Add existing attachment' }));
  fireEvent.click(screen.getByRole('button', { name: 'Restore to draft' }));
  const dialog = screen.getByRole('dialog');
  expect(field()).toHaveValue('');
  fireEvent.click(within(dialog).getByRole('button', { name: 'Add to current draft' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(field()).toHaveValue('Original message\nwith a second line');
  expect(screen.getByRole('list', { name: 'Attachments' })).toHaveTextContent('keep.txt');
  expect(readAgentDraft('alice', 'agt_1').attachments).toHaveLength(1);
  expect(send).not.toHaveBeenCalled();
});

it('scopes restored drafts and pending confirmations to their owner and Agent', async () => {
  const { field, Surface, rerender, send } = setup();
  fireEvent.click(screen.getByRole('button', { name: 'Restore to draft' }));
  rerender(<Surface agent="agt_2" />);
  expect(field()).toHaveValue('');
  rerender(<Surface />);
  expect(field()).toHaveValue('Original message\nwith a second line');
  fireEvent.click(screen.getByRole('button', { name: 'Restore to draft' }));
  expect(screen.getByRole('dialog')).toBeInTheDocument();
  rerender(<Surface owner="bob" />);
  expect(field()).toHaveValue('');
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(readAgentDraft('bob', 'agt_1').content).toBe('');
  expect(send).not.toHaveBeenCalled();
});

it('keeps in-memory restored text and reports a storage failure without claiming it was saved', () => {
  const { field, send } = setup();
  const failure = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Unavailable');
  });
  try {
    fireEvent.click(screen.getByRole('button', { name: 'Restore to draft' }));
    expect(field()).toHaveValue('Original message\nwith a second line');
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Your draft was not saved to this browser.',
    );
    expect(send).not.toHaveBeenCalled();
  } finally {
    failure.mockRestore();
  }
});

it('does not overwrite unreadable saved drafts during restoration', () => {
  sessionStorage.setItem(agentDraftKey('alice', 'agt_1'), 'unreadable');
  const { field, send } = setup();
  expect(screen.getByRole('button', { name: 'Restore to draft' })).toBeDisabled();
  expect(field()).toHaveValue('');
  expect(sessionStorage.getItem(agentDraftKey('alice', 'agt_1'))).toBe('unreadable');
  expect(send).not.toHaveBeenCalled();
});

it('cancels restoration confirmation when another request becomes pending', async () => {
  const { field, Surface, rerender, send } = setup();
  fireEvent.change(field(), { target: { value: 'Keep this request draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Restore to draft' }));
  rerender(<Surface pending />);
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(field()).toHaveValue('Keep this request draft');
  expect(send).not.toHaveBeenCalled();
});
