import { act, fireEvent, render, renderHook, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { Composer } from './Composer';
import { useAgentDraft } from './draft';
import { agentDraftKey, readAgentDraft } from './draftStorage';
import { agentTransportJournal } from './transport/journal';

afterEach(() => vi.restoreAllMocks());

it('restores exact unsent text after losing all React state and recreating the client', () => {
  const firstClient = agentClient();
  const first = renderHook(() => useAgentDraft(firstClient, 'draft-owner', 'draft-agent'));
  const text = '  草稿 🧪\n\tUnsent text  \n';
  act(() => first.result.current.setContent(text));
  first.unmount();
  const secondClient = agentClient();
  const second = renderHook(() => useAgentDraft(secondClient, 'draft-owner', 'draft-agent'));
  expect(second.result.current.content).toBe(text);
});

it.each([
  ['Original message', ''],
  ['Edited draft', 'Edited draft'],
])('retains only unaccepted input across composer remounts: %s', (content, expected) => {
  const client = agentClient();
  let accept: (() => void) | undefined;
  function Surface({ visible, accepted }: { visible: boolean; accepted?: string }) {
    const draft = useAgentDraft(client, 'owner', 'agt_1');
    return visible ? (
      <Composer
        draft={draft}
        status="ready"
        running={false}
        busy={false}
        acceptedMessage={accepted}
        inputAvailability={{ allowed: true }}
        onSend={(_message, onAccepted) => {
          accept = onAccepted;
        }}
        onStop={vi.fn()}
      />
    ) : null;
  }
  const view = render(<Surface visible />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Original message' } });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  if (content !== 'Original message') fireEvent.change(field, { target: { value: content } });
  act(() => accept?.());
  view.rerender(<Surface visible accepted="Original message" />);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(expected);
  view.rerender(<Surface visible={false} accepted="Original message" />);
  view.rerender(<Surface visible accepted="Original message" />);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(expected);
});

it('ignores late acceptance from a previous visit to the same Agent', () => {
  const client = agentClient();
  let accept: (() => void) | undefined;
  function Surface({ agentId }: { agentId: string }) {
    const draft = useAgentDraft(client, 'owner', agentId);
    return (
      <Composer
        key={agentId}
        draft={draft}
        status="ready"
        running={false}
        busy={false}
        inputAvailability={{ allowed: true }}
        onSend={(_content, onAccepted) => {
          accept = onAccepted;
        }}
        onStop={vi.fn()}
      />
    );
  }
  const view = render(<Surface agentId="agt_1" />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Same text, new intent' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(accept).toBeDefined();
  view.rerender(<Surface agentId="agt_2" />);
  view.rerender(<Surface agentId="agt_1" />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Same text, new intent' },
  });
  act(() => accept?.());
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Same text, new intent');
});

it.each(['quota', 'denied'])(
  'keeps exact current text and a recovery action on storage %s',
  (failure) => {
    const client = agentClient();
    const hook = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
    const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new DOMException(failure, failure === 'quota' ? 'QuotaExceededError' : 'SecurityError');
    });
    act(() => hook.result.current.setContent('  Keep this text\n草稿  '));
    expect(hook.result.current.content).toBe('  Keep this text\n草稿  ');
    expect(hook.result.current.error).toMatch(/Copy your text/);
    set.mockRestore();
    act(() => hook.result.current.retryStorage?.());
    expect(hook.result.current.error).toBeUndefined();
    expect(readAgentDraft('owner', 'agt_1').content).toBe('  Keep this text\n草稿  ');
  },
);

it.each(['{', '{"version":99}'])(
  'preserves a damaged record and current edits until explicit discard: %s',
  (raw) => {
    const client = agentClient();
    const setup = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
    setup.unmount();
    sessionStorage.setItem(agentDraftKey('owner', 'agt_1'), raw);
    const hook = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
    expect(hook.result.current.error).toMatch(/could not be recovered/);
    act(() => hook.result.current.setContent('Current text'));
    expect(sessionStorage.getItem(agentDraftKey('owner', 'agt_1'))).toBe(raw);
    expect(hook.result.current.content).toBe('Current text');
    act(() => hook.result.current.discardSaved?.());
    expect(hook.result.current.error).toBeUndefined();
    expect(readAgentDraft('owner', 'agt_1').content).toBe('Current text');
  },
);

it.each([false, true])(
  'uses the retained command key to clear after remount, preserving later identical intent=%s',
  (edited) => {
    const client = agentClient();
    const first = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
    act(() => first.result.current.setContent('Same text'));
    const submission = first.result.current.beginSubmission!('Same text');
    const journal = agentTransportJournal(sessionStorage, 'owner', 'agt_1');
    journal.select(true);
    const intent = journal.retain('Same text');
    submission.settled();
    expect(readAgentDraft('owner', 'agt_1').commandKey).toBe(intent.key);
    first.unmount();
    const second = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
    if (edited) act(() => second.result.current.setContent('Same text'));
    act(() => journal.accepted(intent.key, { commandId: 'cmd_a', responseId: 'resp_a' }));
    expect(second.result.current.content).toBe(edited ? 'Same text' : '');
    expect(readAgentDraft('owner', 'agt_1').content).toBe(edited ? 'Same text' : '');
  },
);

it('does not clear an unsent draft merely because an earlier accepted command has identical text', () => {
  const client = agentClient();
  const hook = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
  act(() => hook.result.current.setContent('Identical text'));
  const journal = agentTransportJournal(sessionStorage, 'owner', 'agt_1');
  journal.select(true);
  const intent = journal.retain('Identical text');
  act(() => journal.accepted(intent.key, { commandId: 'cmd_a', responseId: 'resp_a' }));
  expect(hook.result.current.content).toBe('Identical text');
  expect(readAgentDraft('owner', 'agt_1').content).toBe('Identical text');
});

it('ignores a stale accepted text prop on initial recovery', () => {
  const client = agentClient();
  const hook = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
  act(() => hook.result.current.setContent('Old text, new draft'));
  hook.unmount();
  function Surface({ accepted }: { accepted?: string }) {
    const draft = useAgentDraft(client, 'owner', 'agt_1');
    return (
      <Composer
        draft={draft}
        acceptedMessage={accepted}
        status="ready"
        running={false}
        busy={false}
        inputAvailability={{ allowed: true }}
        onSend={vi.fn()}
        onStop={vi.fn()}
      />
    );
  }
  const view = render(<Surface />);
  view.rerender(<Surface accepted="Old text, new draft" />);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Old text, new draft');
});

it('finishes interrupted accepted-draft cleanup from the same authoritative journal key', () => {
  const client = agentClient();
  const hook = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
  act(() => hook.result.current.setContent('Accepted input'));
  const submission = hook.result.current.beginSubmission!('Accepted input');
  const journal = agentTransportJournal(sessionStorage, 'owner', 'agt_1');
  journal.select(true);
  const intent = journal.retain('Accepted input');
  submission.settled();
  const remove = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
    throw new DOMException('Temporarily denied', 'SecurityError');
  });
  act(() => journal.accepted(intent.key, { commandId: 'cmd_a', responseId: 'resp_a' }));
  expect(hook.result.current.content).toBe('');
  expect(hook.result.current.error).toMatch(/accepted.*could not be removed/);
  expect(journal.read()?.state).toBe('accepted');
  hook.unmount();
  remove.mockRestore();
  const reloaded = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
  act(() => journal.reconcileDraft());
  expect(reloaded.result.current.content).toBe('');
  expect(sessionStorage.getItem(agentDraftKey('owner', 'agt_1'))).toBeNull();
});

it('keeps current edits when recovery of an unreadable draft fails again', () => {
  const client = agentClient();
  const first = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
  first.unmount();
  sessionStorage.setItem(agentDraftKey('owner', 'agt_1'), '{');
  const hook = renderHook(() => useAgentDraft(client, 'owner', 'agt_1'));
  act(() => hook.result.current.setContent('Text typed during recovery failure'));
  act(() => hook.result.current.retryStorage?.());
  expect(hook.result.current.content).toBe('Text typed during recovery failure');
  expect(hook.result.current.recoveryBlocked).toBe(true);
});

it('does not treat identical accepted text from another command as proof that this submission succeeded', () => {
  const client = agentClient();
  function Surface({ accepted }: { accepted?: string }) {
    const draft = useAgentDraft(client, 'owner', 'agt_1');
    return (
      <Composer
        draft={draft}
        acceptedMessage={accepted}
        status="ready"
        running={false}
        busy={false}
        inputAvailability={{ allowed: true }}
        onSend={() => {}}
        onStop={vi.fn()}
      />
    );
  }
  const view = render(<Surface />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Repeated message' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  view.rerender(<Surface accepted="Repeated message" />);
  expect(readAgentDraft('owner', 'agt_1').content).toBe('Repeated message');
});
