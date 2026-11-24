import { useRef } from 'react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MessageStream } from './MessageStream';
import { Composer } from './Composer';
import type { ComposerSuggestionEditorHandle } from './ComposerSuggestionEditor';
import { useAgentDraft } from './draft';
import { agentClient } from '../test/agent';
import { readAgentDraft } from './draftStorage';
import { turnTimeoutFixture } from './turnTimeout.fixture';
import { turnRetryInput } from './turnRetryInput';
import { attachmentMarker } from './attachments';

beforeEach(() => sessionStorage.clear());

function setup({ existing = '', pending = false, readOnly = false } = {}) {
  const client = agentClient();
  const send = vi.fn();
  const conversation = turnTimeoutFixture();
  function Surface() {
    const draft = useAgentDraft(client, 'owner', 'agent-fixture');
    const editor = useRef<ComposerSuggestionEditorHandle>(null);
    return (
      <>
        <MessageStream
          conversation={conversation}
          onApproval={vi.fn()}
          approvalReadOnly={readOnly}
          onReviewRetry={pending ? undefined : (text) => editor.current?.restore(text, () => {})}
        />
        <button
          onClick={() =>
            draft.setAttachments([
              {
                key: 'keep',
                status: 'reselect',
                description: { name: 'keep.txt', size: 2, mediaType: 'text/plain' },
              },
            ])
          }
        >
          Add existing attachment
        </button>
        <Composer
          editorRef={editor}
          draft={draft}
          status="ready"
          running={false}
          busy={false}
          requestPending={pending}
          inputAvailability={{ allowed: !readOnly }}
          onSend={send}
          onStop={vi.fn()}
        />
      </>
    );
  }
  const view = render(<Surface />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  if (existing) fireEvent.change(field, { target: { value: existing } });
  return { ...view, field, send, client, Surface };
}

it('prepares the complete recorded input and sends only on a new explicit Send', () => {
  const { field, send } = setup();
  expect(screen.getByText('This turn reached its time limit and has ended.')).toBeVisible();
  expect(screen.getByText(/Repeating a task may repeat effects/)).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Review and retry' }));
  expect(field).toHaveValue('Check the draft');
  expect(field).toHaveFocus();
  expect(send).not.toHaveBeenCalled();
  expect(readAgentDraft('owner', 'agent-fixture').commandKey).toBeUndefined();
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(send).toHaveBeenCalledExactlyOnceWith('Check the draft', expect.any(Function));
});

it('keeps an existing draft and attachments until an explicit append decision', () => {
  const { field, send } = setup({ existing: 'Keep my draft' });
  fireEvent.click(screen.getByRole('button', { name: 'Add existing attachment' }));
  fireEvent.click(screen.getByRole('button', { name: 'Review and retry' }));
  const dialog = screen.getByRole('dialog', { name: 'Keep your draft?' });
  expect(field).toHaveValue('Keep my draft');
  expect(screen.getByText('keep.txt')).toBeVisible();
  expect(within(dialog).getByText(/Nothing will be sent/)).toBeVisible();
  fireEvent.click(within(dialog).getByRole('button', { name: 'Keep editing' }));
  expect(field).toHaveValue('Keep my draft');
  fireEvent.click(screen.getByRole('button', { name: 'Review and retry' }));
  fireEvent.click(
    within(screen.getByRole('dialog')).getByRole('button', { name: 'Add to current draft' }),
  );
  expect(field).toHaveValue('Keep my draft\nCheck the draft');
  expect(readAgentDraft('owner', 'agent-fixture').attachments).toHaveLength(1);
  expect(send).not.toHaveBeenCalled();
});

it.each([{ pending: true }, { readOnly: true }])(
  'does not offer retry under a pending/unauthorized gate %#',
  (options) => {
    const { send } = setup(options);
    expect(screen.queryByRole('button', { name: 'Review and retry' })).toBeNull();
    expect(send).not.toHaveBeenCalled();
  },
);

it('does not silently drop original attachments or reconstruct missing input', () => {
  const attached = turnTimeoutFixture(`Check files${attachmentMarker}[]`);
  expect(turnRetryInput(attached, '1')).toEqual({
    unavailable: expect.stringContaining('attachments need to be checked'),
  });
  const missing = { ...turnTimeoutFixture(), messages: [] };
  expect(turnRetryInput(missing, '1')).toEqual({
    unavailable: expect.stringContaining('complete original message is unavailable'),
  });
  render(<MessageStream conversation={missing} onApproval={vi.fn()} onReviewRetry={vi.fn()} />);
  expect(screen.getByText(/complete original message is unavailable/)).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Review and retry' })).toBeNull();
});

it('does not prepare a stale turn or a partial/unconfirmed original message', () => {
  const original = turnTimeoutFixture();
  expect(turnRetryInput({ ...original, activeRunId: '5', latestTurnId: '5' }, '1')).toBeUndefined();
  expect(turnRetryInput({ ...original, latestTurnId: '5' }, '1')).toBeUndefined();
  const partial = {
    ...original,
    messages: original.messages.map((item) => ({ ...item, status: 'streaming' as const })),
  };
  expect(turnRetryInput(partial, '1')).toHaveProperty('unavailable');
});
