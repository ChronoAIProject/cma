import { expect, it, vi } from 'vitest';
import type { ConversationState } from './model';
import type { OptimisticMessage } from './optimisticMessages';
import type { QueuedInputsControl } from './transport/queuedInputsStatus';
import { queuePresentation } from './queuePresentation';

const conversation: ConversationState = {
  messages: [],
  toolCalls: [],
  reasoning: [],
  approvals: [],
  recovery: [],
  cursor: 0,
};
const active = (commandId: string): ConversationState => ({
  ...conversation,
  activeRunId: '1',
  responseBinding: { commandId, responseId: `resp_${commandId}`, turnSequence: 1 },
});
const row = (commandId: string) => ({
  commandId,
  responseId: `resp_${commandId}`,
  revision: 7,
  input: `Message ${commandId} /home/attachments/${commandId}.png`,
});
const queue = (ids: string[]): QueuedInputsControl => ({
  items: ids.map(row),
  managedCommandIds: new Set(ids),
  edit: vi.fn(),
  remove: vi.fn(),
  recover: vi.fn(),
  refresh: vi.fn(),
});
const message = (id: string, queued?: boolean): OptimisticMessage => ({
  id,
  content: row(id).input,
  queued,
  state: 'accepted',
  receipt: { commandId: id, responseId: row(id).responseId },
});

it('keeps real queue entries across idle handoff and displays the starting entry once with its attachments', () => {
  const control = queue(['b', 'c']);
  const input = [message('b', true), message('c', true)];
  for (const state of [active('a'), conversation]) {
    const result = queuePresentation(state, input, control);
    expect(result.items).toEqual(control.items);
    expect(result.messages).toEqual([]);
  }
  const starting = queuePresentation(active('b'), input, control);
  expect(starting.items).toEqual([row('c')]);
  expect(starting.messages).toEqual([message('b', false)]);
  const delivered = queuePresentation(
    {
      ...active('b'),
      messages: [
        {
          id: 'event:2',
          role: 'user',
          content: row('b').input,
          status: 'complete',
          commandId: 'b',
        },
      ],
    },
    [],
    control,
  );
  expect(delivered.items).toEqual([row('c')]);
  expect(delivered.messages).toEqual([]);
});

it('preserves recovered queue entries and bridges their own start before the durable user frame', () => {
  const control = queue(['b', 'c']);
  expect(queuePresentation(conversation, [message('c')], control).items).toEqual(control.items);
  const started = queuePresentation(active('b'), [], control);
  expect(started.items).toEqual([row('c')]);
  expect(started.messages).toHaveLength(1);
  expect(started.messages[0]).toMatchObject({
    content: row('b').input,
    queued: false,
    receipt: { commandId: 'b', responseId: 'resp_b' },
  });
});

it('does not resurrect cancelled ordinary input or hide uncertain queue mutations', () => {
  const control = queue(['b']);
  expect(
    queuePresentation(conversation, [message('b', false)], { ...control, items: [] }).messages,
  ).toEqual([]);
  const recovery = { ...control, items: [{ ...row('b'), needsRecovery: true }] };
  const result = queuePresentation(active('b'), [message('b', false)], recovery);
  expect(result.items).toEqual(recovery.items);
  expect(result.messages).toEqual([]);
});

it('does not use a previous turn binding to remove another queued entry', () => {
  const control = queue(['b']);
  const stale = { ...active('b'), activeRunId: '2' };
  expect(queuePresentation(stale, [], control).items).toEqual(control.items);
});

it('retains known queues and recovery while deferring new discovery until an idle Send settles', () => {
  const control = queue(['older', 'new']);
  const pending: OptimisticMessage = {
    id: 'local',
    content: 'Same text is not identity',
    state: 'sending',
    queued: false,
    queueBeforeSend: ['older'],
  };
  expect(queuePresentation(conversation, [pending], control).items).toEqual([row('older')]);
  expect(
    queuePresentation(conversation, [{ ...pending, state: 'unconfirmed' }], control).items,
  ).toEqual(control.items);
  const recovery = {
    ...control,
    items: [...control.items, { ...row('recovery'), needsRecovery: true }],
  };
  expect(
    queuePresentation(conversation, [pending], recovery).items.map((item) => item.commandId),
  ).toEqual(['older', 'recovery']);
});

it('preserves an unconfirmed input without synthesizing another bubble for its unknown command', () => {
  const pending: OptimisticMessage = {
    id: 'local',
    content: 'Unconfirmed input /home/attachments/local.png',
    state: 'unconfirmed',
    queued: false,
    queueBeforeSend: ['older'],
  };
  const control = queue(['older', 'new']);
  const ownStart = queuePresentation(active('new'), [pending], control);
  expect(ownStart.messages).toEqual([pending]);
  expect(ownStart.items).toEqual([row('older')]);

  // An already known queue entry has independent identity and remains visible.
  const previousStart = queuePresentation(active('older'), [pending], control);
  expect(previousStart.messages).toHaveLength(2);
  expect(previousStart.messages[0]).toEqual(pending);
  expect(previousStart.messages[1]).toMatchObject({
    content: row('older').input,
    receipt: { commandId: 'older', responseId: 'resp_older' },
  });
  expect(previousStart.items).toEqual([row('new')]);
});
