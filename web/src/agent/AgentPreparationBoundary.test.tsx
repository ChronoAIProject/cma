import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import type { AgentLifecycleOperationView } from '../api/client';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import type { Failure } from '../api/types';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { lifecycleRecoveryKey } from '../create/lifecycleIntentStorage';
import { retainPreparationDraftOwner } from '../preparation/preparationDraftIdentity';
import { readPreparationDraft, savePreparationDraft } from '../preparation/preparationDraftStorage';
import { AgentSurface } from './AgentSurface';
import { controller } from './AgentSurface.fixture';
import { conversationFromTranscript } from './conversation';
import { useAgentDraft } from './draft';
import { readAgentDraft, saveAgentDraft } from './draftStorage';
import { retainAgentDraftOwner } from './draftIdentity';
import * as connections from '../connections/preparation';

const original = 'Original task\n  Keep exact spacing';
const text = 'Follow-up draft\nReady only on my send';
const scope = { owner: 'owner', intentKey: 'original-key' };
const binding = {
  operationId: 'operation-one',
  agentId: 'agt_1',
  sandboxId: 'sbx_1',
  workspaceId: 'wsp_1',
  worktreeId: 'wkt_1',
};
const journal = lifecycleRecoveryKey({ owner: 'owner' }, binding);
const queued: AgentLifecycleOperationView = {
  id: 'operation-one',
  status: 'running',
  phase: 'agent_startup',
  agent_id: 'agt_1',
  events: [],
  accepted_at: '2026-11-23T00:00:00Z',
};
beforeEach(() => {
  sessionStorage.clear();
  retainPreparationDraftOwner('owner');
  sessionStorage.setItem(
    journal,
    JSON.stringify({
      version: 1,
      intent: {
        key: scope.intentKey,
        body: { name: 'New task', initial_message: original },
        receipt: { id: queued.id },
        agentId: 'agt_1',
      },
    }),
  );
});
afterEach(() => vi.restoreAllMocks());

function fixture() {
  let operation = queued;
  const create = vi.fn();
  const send = vi.fn();
  const read = vi.fn(async () => operation);
  const client = agentClient({ readAgentLifecycle: read, createAgentLifecycle: create });
  const detail = {
    ...AGENT_DETAIL,
    title: undefined,
    status: 'starting' as const,
    firstMessage: {
      state: 'queued' as const,
      purpose: 'task' as const,
      queuedAt: queued.accepted_at!,
    },
  };
  const connection = vi.spyOn(connections, 'readAgentConnections');
  function Host({
    started = false,
    failure,
    sandboxFailure,
  }: {
    readonly started?: boolean;
    readonly failure?: Failure;
    readonly sandboxFailure?: Failure;
  }) {
    const draft = useAgentDraft(client, 'owner', 'agt_1');
    const state = {
      status: 'ready' as const,
      detail: failure
        ? { ...detail, status: 'failed' as const, failure }
        : started
          ? AGENT_DETAIL
          : detail,
      workspace: workspaceFixture(),
      conversation: conversationFromTranscript(
        started ? AGENT_TRANSCRIPT : { ...AGENT_TRANSCRIPT, turns: [] },
      ),
    };
    return (
      <CmaClientProvider client={client}>
        <AgentSurface
          agentId="agt_1"
          viewerId="owner"
          lifecycleClient={client}
          draft={draft}
          accessAuthorized
          detailsOpen={false}
          onDetailsOpenChange={vi.fn()}
          onNavigate={vi.fn()}
          firstSendContext={{ authEpoch: 7, visible: true }}
          controller={controller(state, undefined, {
            send,
            progressObservation: sandboxFailure
              ? {
                  sandbox: {
                    ...sandboxFixture(),
                    id: detail.sandbox.id,
                    status: 'failed',
                    failure: sandboxFailure,
                  },
                  stale: false,
                }
              : undefined,
          })}
        />
      </CmaClientProvider>
    );
  }
  return {
    Host,
    create,
    send,
    read,
    connection,
    succeed: () => {
      operation = { ...queued, status: 'succeeded' };
    },
  };
}

it.each(['failure', 'sandboxFailure'] as const)(
  'shows a recorded %s without a browser creation receipt',
  async (field) => {
    sessionStorage.removeItem(journal);
    const h = fixture();
    const failure = {
      class: 'terminal',
      detail: 'The Agent process could not start.',
    };
    const { container } = render(<h.Host {...{ [field]: failure }} />);
    expect(await screen.findByRole('heading', { name: 'Setup needs attention' })).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent(failure.detail);
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Review its environment before trying again.',
    );
    expect(screen.getByText('Task hasn’t started')).toBeVisible();
    expect(screen.queryByText('Preparing cloud computer')).toBeNull();
    expect(container.querySelectorAll('[data-motion="live"]')).toHaveLength(0);
  },
);

it('uses one bound preparation view and transfers only the follow-up into the mounted Agent draft after real activity', async () => {
  const source = savePreparationDraft(scope, undefined, text, binding);
  const h = fixture();
  const view = render(<h.Host />);
  expect(
    await screen.findByRole('heading', { name: 'Getting its cloud computer ready' }),
  ).toBeVisible();
  expect(screen.getAllByRole('heading', { name: 'New agent' })).toHaveLength(1);
  expect(screen.getByRole('region', { name: 'Original request' }).textContent).toBe(original);
  expect(screen.getAllByText('Setup details')).toHaveLength(1);
  expect(screen.getByLabelText('Follow-up draft')).toHaveValue(text);
  expect(screen.queryByRole('textbox', { name: 'Message' })).not.toBeInTheDocument();
  expect(view.container.querySelectorAll('[data-motion="live"]')).toHaveLength(1);
  fireEvent.keyDown(screen.getByLabelText('Follow-up draft'), { key: 'Enter', ctrlKey: true });
  expect(h.connection).not.toHaveBeenCalled();
  expect(h.create).not.toHaveBeenCalled();
  expect(h.send).not.toHaveBeenCalled();
  expect(readPreparationDraft(scope)).toEqual(source);
  h.succeed();
  view.rerender(<h.Host started />);
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(text));
  await waitFor(() => expect(sessionStorage.getItem(journal)).toBeNull());
  expect(readPreparationDraft(scope)).toBeUndefined();
  expect(readAgentDraft('owner', 'agt_1').content).toBe(text);
  expect(screen.queryByRole('region', { name: 'Task preparation' })).not.toBeInTheDocument();
  expect(h.create).not.toHaveBeenCalled();
  expect(h.send).not.toHaveBeenCalled();
});

it('discovers a bound draft after lifecycle cleanup and refuses to replace an independent message', async () => {
  const source = savePreparationDraft(scope, undefined, text, binding);
  sessionStorage.removeItem(journal);
  retainAgentDraftOwner('owner');
  saveAgentDraft('owner', 'agt_1', {
    content: 'Independent message',
    attachments: [],
    revision: 'existing-revision',
  });
  const h = fixture();
  const view = render(<h.Host started />);
  expect(await screen.findByRole('textbox', { name: 'Message' })).toHaveValue(
    'Independent message',
  );
  expect(screen.getByLabelText('Saved follow-up draft')).toHaveValue(text);
  expect(screen.getByRole('button', { name: 'Move draft to message' })).toBeDisabled();
  expect(readPreparationDraft(scope)).toEqual(source);
  expect(h.read).not.toHaveBeenCalled();
  view.unmount();
  render(<h.Host started />);
  expect(await screen.findByLabelText('Saved follow-up draft')).toHaveValue(text);
  expect(readPreparationDraft(scope)).toEqual(source);
  expect(h.send).not.toHaveBeenCalled();
});

it('keeps the Agent draft untouched while the accepted original request is still being prepared', async () => {
  const h = fixture();
  render(<h.Host />);
  const input = await screen.findByLabelText('Follow-up draft');
  fireEvent.change(input, { target: { value: text } });
  await act(async () => {});
  expect(readAgentDraft('owner', 'agt_1').content).toBe('');
  expect(readPreparationDraft(scope)?.text).toBe(text);
  expect(readPreparationDraft(scope)?.binding).toEqual(binding);
  expect(sessionStorage.getItem(journal)).toContain(original.replaceAll('\n', '\\n'));
  expect(h.create).not.toHaveBeenCalled();
  expect(h.send).not.toHaveBeenCalled();
});
