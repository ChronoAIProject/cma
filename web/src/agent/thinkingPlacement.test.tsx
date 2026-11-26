import { act, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ActiveThinking } from './ActiveThinking';
import { conversationFromTranscript } from './conversation';
import { observeAnimations } from '../test/animations';

let motion: ReturnType<typeof observeAnimations>;
beforeEach(() => {
  motion = observeAnimations();
});
afterEach(() => {
  motion.restore();
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const active = () => ({
  ...conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
  activeRunId: '1',
});

it('eases only measured status movement, preserves its node and cancels on reduced motion', () => {
  const state = active();
  const { container, rerender, unmount } = render(<ActiveThinking conversation={state} />);
  const row = container.querySelector<HTMLElement>('.active-thinking')!;
  let top = 140;
  Object.defineProperty(row, 'offsetTop', { get: () => top });
  rerender(<ActiveThinking conversation={{ ...state }} />);
  expect(motion.records).toHaveLength(1);
  expect(motion.records[0]?.element).toBe(row);
  expect(motion.records[0]?.frames).toEqual([
    { transform: 'translateY(-140px)' },
    { transform: 'translateY(0)' },
  ]);
  expect(motion.records[0]?.options).toMatchObject({ duration: 200 });
  rerender(<ActiveThinking conversation={{ ...state }} />);
  expect(motion.records).toHaveLength(1);
  expect(container.querySelector('.active-thinking')).toBe(row);
  act(() => motion.reduce(true));
  expect(motion.records[0]?.animation.cancel).toHaveBeenCalled();
  top = 180;
  rerender(<ActiveThinking conversation={{ ...state }} />);
  expect(motion.records).toHaveLength(1);
  act(() => motion.reduce(false));
  top = 220;
  rerender(<ActiveThinking conversation={{ ...state }} />);
  expect(motion.records).toHaveLength(2);
  unmount();
  expect(motion.records[1]?.animation.cancel).toHaveBeenCalledOnce();
});

it('releases interaction immediately and removes the collapsed exit instead of leaving a spacer', () => {
  vi.useFakeTimers();
  const { container, rerender } = render(<ActiveThinking conversation={active()} />);
  rerender(
    <ActiveThinking
      conversation={conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] })}
    />,
  );
  expect(screen.queryByRole('button', { name: 'View activity' })).not.toBeInTheDocument();
  expect(container.querySelector('.active-thinking')).toHaveAttribute('inert');
  act(() => vi.advanceTimersByTime(200));
  expect(container.querySelector('.active-thinking')).toBeNull();
});

it('removes the status immediately with reduced motion and never replays exit status', () => {
  vi.useFakeTimers();
  act(() => motion.reduce(true));
  const { container, rerender } = render(<ActiveThinking conversation={active()} />);
  const finished = conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
  rerender(<ActiveThinking conversation={finished} />);
  expect(container.querySelector('.active-thinking')).toBeNull();
  act(() => motion.reduce(false));
  rerender(<ActiveThinking conversation={finished} />);
  expect(container.querySelector('.active-thinking')).toBeNull();
});

it('shows optimistic stopping feedback without claiming a terminal turn', () => {
  const { rerender } = render(<ActiveThinking conversation={active()} stopping />);
  expect(screen.getByText('Stopping…')).toBeVisible();
  expect(screen.queryByText(/Response interrupted/)).not.toBeInTheDocument();

  const interrupted = {
    ...conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
    latestTurnId: '1',
    turnOutcomes: { '1': 'interrupted' as const },
  };
  rerender(<ActiveThinking conversation={interrupted} stopping />);
  expect(screen.getByRole('status')).toHaveTextContent(
    'Response interrupted. Running Sandbox commands may continue.',
  );
  expect(screen.queryByText('Stopping…')).not.toBeInTheDocument();
});
