import { describe, expect, it } from 'vitest';
import type { ConversationMessage, ToolCall } from './model';
import { groupToolCalls, type Entry } from './timeline';

const tool = (id: string, status = 'completed'): Entry => ({
  kind: 'tool',
  item: { id, title: `Call ${id}`, status } satisfies ToolCall,
});
const message = (id: string): Entry => ({
  kind: 'message',
  item: {
    id,
    role: 'assistant',
    content: id,
    status: 'complete',
  } satisfies ConversationMessage,
});

describe('groupToolCalls', () => {
  it('returns nothing for an empty timeline', () => {
    expect(groupToolCalls([])).toEqual([]);
  });

  it('keeps a lone tool call as its own row', () => {
    const only = tool('a');
    expect(groupToolCalls([only])).toEqual([only]);
  });

  it('groups only consecutive calls and leaves messages where they were', () => {
    const items = groupToolCalls([
      message('m1'),
      tool('a'),
      tool('b'),
      message('m2'),
      tool('c'),
      tool('d'),
      tool('e'),
    ]);
    expect(items.map((item) => item.kind)).toEqual(['message', 'tools', 'message', 'tools']);
    expect(items[1]).toMatchObject({ id: 'tools:a', failed: false });
    expect(items[1]?.kind === 'tools' && items[1].items.map(({ id }) => id)).toEqual(['a', 'b']);
    expect(items[3]?.kind === 'tools' && items[3].items.map(({ id }) => id)).toEqual([
      'c',
      'd',
      'e',
    ]);
  });

  it('does not join calls separated by a single message into one group', () => {
    expect(groupToolCalls([tool('a'), message('m'), tool('b')]).map((item) => item.kind)).toEqual([
      'tool',
      'message',
      'tool',
    ]);
  });

  it('marks a group holding a failure so it starts expanded', () => {
    const [group] = groupToolCalls([tool('a'), tool('b', 'Failed'), tool('c', 'running')]);
    expect(group).toMatchObject({ kind: 'tools', failed: true });
  });

  it('does not treat an unreported or cancelled outcome as a failure', () => {
    const [group] = groupToolCalls([tool('a', 'running'), tool('b', 'cancelled')]);
    expect(group).toMatchObject({ kind: 'tools', failed: false });
  });
});
