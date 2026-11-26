import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AgentDetail } from '../api/types';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { useAgentDraft } from '../agent/draft';
import { readAgentDraft, saveAgentDraft } from '../agent/draftStorage';
import { retainAgentDraftOwner } from '../agent/draftIdentity';
import { PreparationDraftRecovery } from './PreparationDraftRecovery';
import { retainPreparationDraftOwner } from './preparationDraftIdentity';
import {
  preparationDraftKey,
  readPreparationDraft,
  savePreparationDraft,
} from './preparationDraftStorage';

const scope = { owner: 'owner', intentKey: 'original-key' };
const binding = {
  operationId: 'original-operation',
  agentId: 'agt_1',
  sandboxId: 'sbx_1',
  workspaceId: 'wsp_1',
  worktreeId: 'wkt_1',
};
const text = 'Saved follow-up\nnot an original task';
beforeEach(() => {
  sessionStorage.clear();
  retainPreparationDraftOwner('owner');
  retainAgentDraftOwner('owner');
});
afterEach(() => vi.restoreAllMocks());
function fixture() {
  const source = savePreparationDraft(scope, undefined, text, binding);
  const client = agentClient({ runAgent: vi.fn(), createAgentLifecycle: vi.fn() });
  function Host({
    authorized = true,
    detail = AGENT_DETAIL,
  }: {
    readonly authorized?: boolean;
    readonly detail?: AgentDetail;
  }) {
    const draft = useAgentDraft(client, 'owner', 'agt_1');
    return (
      <>
        <textarea
          aria-label="Destination"
          value={draft.content}
          onChange={(event) => draft.setContent(event.target.value)}
        />
        <PreparationDraftRecovery
          client={client}
          viewerId="owner"
          detail={detail}
          draft={draft}
          authorized={authorized}
          epoch={7}
        />
      </>
    );
  }
  return { source, client, Host };
}

it('does not reveal or transfer a draft until the current pending/authorization fence allows it', async () => {
  const h = fixture();
  const view = render(<h.Host authorized={false} />);
  expect(screen.queryByLabelText('Saved follow-up draft')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Destination')).toHaveValue('');
  expect(readPreparationDraft(scope)).toEqual(h.source);
  view.rerender(<h.Host />);
  await waitFor(() => expect(screen.getByLabelText('Destination')).toHaveValue(text));
  expect(readPreparationDraft(scope)).toBeUndefined();
  expect(h.client.runAgent).not.toHaveBeenCalled();
  expect(h.client.createAgentLifecycle).not.toHaveBeenCalled();
});

it.each(['worktree', 'workspace', 'sandbox'] as const)(
  'keeps a %s mismatch source without revealing or transferring it',
  (kind) => {
    const h = fixture();
    const detail = { ...AGENT_DETAIL, [kind]: { ...AGENT_DETAIL[kind], id: 'different-parent' } };
    render(<h.Host detail={detail} />);
    expect(screen.queryByLabelText('Saved follow-up draft')).not.toBeInTheDocument();
    expect(screen.getByText(/belongs to a different environment/)).toBeVisible();
    expect(screen.getByLabelText('Destination')).toHaveValue('');
    expect(readPreparationDraft(scope)).toEqual(h.source);
  },
);

it('preserves the exact command-bound destination and source', () => {
  const h = fixture();
  const target = {
    revision: 'original-revision',
    content: 'Unknown original message',
    commandKey: 'original-command-key',
    attachments: [],
  };
  saveAgentDraft('owner', 'agt_1', target);
  render(<h.Host />);
  expect(screen.getByLabelText('Destination')).toHaveValue(target.content);
  expect(screen.getByLabelText('Saved follow-up draft')).toHaveValue(text);
  expect(screen.getByRole('button', { name: 'Move draft to message' })).toBeDisabled();
  expect(readAgentDraft('owner', 'agt_1')).toEqual(target);
  expect(readPreparationDraft(scope)).toEqual(h.source);
});

it('moves a retained follow-up explicitly after the owner clears a conflicting message', async () => {
  const h = fixture();
  saveAgentDraft('owner', 'agt_1', {
    revision: 'conflicting-revision',
    content: 'Independent message draft',
    attachments: [],
  });
  render(<h.Host />);
  expect(screen.getByRole('button', { name: 'Move draft to message' })).toBeDisabled();
  fireEvent.change(screen.getByLabelText('Destination'), { target: { value: '' } });
  expect(screen.getByLabelText('Destination')).toHaveValue('');
  expect(readAgentDraft('owner', 'agt_1').content).toBe('');
  expect(readPreparationDraft(scope)).toEqual(h.source);
  expect(screen.getByRole('button', { name: 'Move draft to message' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Move draft to message' }));
  await waitFor(() => expect(screen.getByLabelText('Destination')).toHaveValue(text));
  expect(readAgentDraft('owner', 'agt_1').content).toBe(text);
  expect(readPreparationDraft(scope)).toBeUndefined();
  expect(h.client.runAgent).not.toHaveBeenCalled();
  expect(h.client.createAgentLifecycle).not.toHaveBeenCalled();
});

it('retains both copies if source removal fails after destination verification', async () => {
  const h = fixture();
  const original = Storage.prototype.removeItem;
  vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(function (this: Storage, key) {
    if (key === preparationDraftKey(scope)) throw new Error('Removal blocked');
    original.call(this, key);
  });
  render(<h.Host />);
  await waitFor(() => expect(screen.getByLabelText('Destination')).toHaveValue(text));
  expect(screen.getByLabelText('Saved follow-up draft')).toHaveValue(text);
  expect(screen.getByRole('button', { name: 'Move draft to message' })).toBeDisabled();
  expect(readPreparationDraft(scope)).toEqual(h.source);
  expect(h.client.runAgent).not.toHaveBeenCalled();
});

it('permits explicit recovery after a read failure without replacing the original source', async () => {
  const h = fixture();
  const original = Storage.prototype.getItem;
  const get = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (
    this: Storage,
    key,
  ) {
    if (key === preparationDraftKey(scope)) throw new Error('Unreadable');
    return original.call(this, key);
  });
  render(<h.Host />);
  expect(screen.getByRole('alert')).toHaveTextContent('could not be read');
  expect(screen.getByLabelText('Destination')).toHaveValue('');
  get.mockRestore();
  fireEvent.click(screen.getByRole('button', { name: 'Retry reading saved drafts' }));
  await waitFor(() => expect(screen.getByLabelText('Destination')).toHaveValue(text));
  expect(readPreparationDraft(scope)).toBeUndefined();
});
