import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { Composer } from './Composer';
import { conversationFromTranscript, foldAgentEvent, foldLiveEvent } from './conversation';
import { foldV2Item } from './transport/projection';
import { sessionCommands } from './slashCommands';
import type { V2AgentCursor } from '@chronoai/cma-sdk';

const commands = [
  { name: 'goal', description: 'Set a persistent goal.' },
  { name: 'status', description: 'Inspect this session.' },
  { name: '$research', description: 'Use the installed research skill.' },
];
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function setup(availableCommands: typeof commands | undefined = commands) {
  const onSend = vi.fn((_text: string, accepted: () => void) => accepted());
  const props = {
    status: 'ready' as const,
    running: false,
    busy: false,
    inputAvailability: { allowed: true },
    onSend,
    onStop: vi.fn(),
    availableCommands,
  };
  const view = render(<Composer {...props} />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  return { ...view, field, onSend, props };
}

it('uses the actual session inventory, filters descriptions and retains focus on keyboard selection', async () => {
  const { field, onSend } = setup();
  const user = userEvent.setup();
  await user.type(field, '/');
  expect(screen.getAllByRole('option')).toHaveLength(3);
  expect(screen.queryByRole('option', { name: /compact/ })).toBeNull();
  await user.keyboard('{ArrowDown}{Enter}');
  expect(field).toHaveValue('/status ');
  expect(field).toHaveFocus();
  expect(onSend).not.toHaveBeenCalled();
  await user.keyboard('{Enter}');
  expect(onSend).toHaveBeenCalledExactlyOnceWith('/status', expect.any(Function));
  expect(field).toHaveValue('');
  await user.type(field, '/persistent');
  expect(screen.getAllByRole('option')).toHaveLength(1);
  await user.keyboard('{Tab}');
  expect(field).toHaveValue('/goal ');
  await user.type(field, 'Finish the migration{Enter}');
  expect(onSend).toHaveBeenLastCalledWith('/goal Finish the migration', expect.any(Function));
});

it('inserts a native skill mention for pointer/mobile selection and sends that exact invocation', async () => {
  const { field, onSend } = setup();
  const user = userEvent.setup();
  await user.type(field, '/research');
  await user.click(
    screen.getByRole('option', { name: '$research Use the installed research skill.' }),
  );
  expect(field).toHaveValue('$research ');
  expect(field).toHaveFocus();
  await user.type(field, 'Review this source{Enter}');
  expect(onSend).toHaveBeenCalledExactlyOnceWith(
    '$research Review this source',
    expect.any(Function),
  );
});

it('keeps unsupported commands as drafts instead of sending ordinary model text', async () => {
  const { field, onSend } = setup();
  const user = userEvent.setup();
  await user.type(field, '/unsupported argument{Enter}');
  expect(onSend).not.toHaveBeenCalled();
  expect(field).toHaveValue('/unsupported argument');
  expect(screen.getByRole('alert')).toHaveTextContent('not available for this Agent');
  await user.clear(field);
  await user.type(field, 'Explain /unsupported and /tmp/files{Enter}');
  expect(onSend).toHaveBeenCalledExactlyOnceWith(
    'Explain /unsupported and /tmp/files',
    expect.any(Function),
  );
});

it('does not invent a catalogue before the session reports one, and reacts to replacement inventories', () => {
  const { field, props, rerender } = setup([]);
  rerender(<Composer {...props} availableCommands={undefined} />);
  fireEvent.change(field, { target: { value: '/' } });
  expect(screen.queryAllByRole('option')).toHaveLength(0);
  expect(screen.getByRole('status')).toHaveTextContent('not reported available commands');
  rerender(<Composer {...props} availableCommands={commands} />);
  expect(screen.getAllByRole('option')).toHaveLength(3);
  rerender(<Composer {...props} availableCommands={[commands[1]!]} />);
  expect(screen.getAllByRole('option')).toHaveLength(1);
});

it('honors Escape and IME Enter, and releases an inert popup on reduced motion', () => {
  vi.spyOn(window, 'matchMedia').mockReturnValue({
    matches: true,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  } as unknown as MediaQueryList);
  const { field, onSend } = setup();
  fireEvent.change(field, { target: { value: '/' } });
  fireEvent.compositionStart(field);
  fireEvent.keyDown(field, { key: 'Enter', isComposing: true });
  expect(field).toHaveValue('/');
  expect(onSend).not.toHaveBeenCalled();
  fireEvent.compositionEnd(field);
  fireEvent.keyDown(field, { key: 'Escape' });
  expect(screen.queryByRole('listbox')).toBeNull();
  expect(document.querySelector('.composer-commands')).toBeNull();
  expect(field).toHaveValue('/');
  expect(field).toHaveFocus();
});

it('cancels stale exit removal when the slash popup is reopened', () => {
  vi.useFakeTimers();
  const { field } = setup();
  fireEvent.change(field, { target: { value: '/' } });
  const panel = screen.getByRole('listbox');
  fireEvent.keyDown(field, { key: 'Escape' });
  expect(panel).toHaveAttribute('inert');
  act(() => vi.advanceTimersByTime(100));
  fireEvent.change(field, { target: { value: '/go' } });
  act(() => vi.advanceTimersByTime(200));
  expect(screen.getByRole('listbox')).toBe(panel);
  expect(panel).not.toHaveAttribute('inert');
});

it('restores the same command inventory from legacy public events, AG-UI and Responses v2 replay', () => {
  const blank = conversationFromTranscript({ agentId: 'agt_1', turns: [], throughSeq: 0 });
  const legacy = foldAgentEvent(blank, {
    seq: 1,
    appendedAt: '2026-10-16T00:00:00Z',
    kind: { type: 'available_commands_set', commands },
  });
  const live = foldLiveEvent(
    blank,
    {
      type: 'STATE_DELTA',
      threadId: 'agt_1',
      runId: 'run_1',
      delta: [{ op: 'replace', path: '/availableCommands', value: commands }],
    },
    'agt_1',
  );
  const v2 = foldV2Item(blank, {
    sequence: 1,
    cursor: `ag1.${'a'.repeat(64)}.1` as V2AgentCursor,
    appended_at: '2026-10-16T00:00:00Z',
    kind: { type: 'available_commands_set', commands },
    response_binding: null,
  });
  expect(legacy.availableCommands).toEqual(commands);
  expect(live.availableCommands).toEqual(commands);
  expect(v2.availableCommands).toEqual(commands);
  expect(
    foldAgentEvent(legacy, {
      seq: 2,
      appendedAt: '2026-10-16T00:00:01Z',
      kind: { type: 'runtime_generation_retired' },
    }).availableCommands,
  ).toBeUndefined();
});

it('deduplicates and rejects invalid command identities from untrusted metadata', () => {
  expect(
    sessionCommands([
      ...commands,
      commands[0],
      { name: 'bad command', description: 'x' },
      { name: '/not-native', description: 'x' },
      { name: 'missing-description' },
    ]),
  ).toEqual(commands);
});
