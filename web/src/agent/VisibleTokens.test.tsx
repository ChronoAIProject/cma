import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TurnUsage } from './TurnUsage';
import { ContextRing } from './ContextRing';
import { AGENT_USAGE } from '../test/agent';
import type { VisibleTokenEstimate } from './tokenEstimate';

class TokenWorker {
  static instances: TokenWorker[] = [];
  onmessage?: (event: MessageEvent<VisibleTokenEstimate & { readonly basis: string }>) => void;
  onerror?: () => void;
  postMessage = vi.fn();
  terminate = vi.fn();
  constructor() {
    TokenWorker.instances.push(this);
  }
  answer(tokens: number, basis = 'unreported') {
    this.onmessage?.({ data: { tokens, partial: false, basis } } as MessageEvent<
      VisibleTokenEstimate & { readonly basis: string }
    >);
  }
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  TokenWorker.instances = [];
});

it('shows live estimates at most five times a second and replaces them with actual usage without changing totals', async () => {
  vi.useFakeTimers();
  vi.stubGlobal('Worker', TokenWorker);
  const draw = (text: string, actual?: { inputTokens: number; outputTokens: number }) => (
    <>
      <TurnUsage
        visibleText={[{ id: 'reply', text }]}
        usage={actual}
        animateInitial={false}
        visibleAnchor={actual === undefined ? undefined : [{ id: 'reply', text: 'Hello world!' }]}
      />
      <ContextRing usage={AGENT_USAGE} />
    </>
  );
  const view = render(draw('Hello'));
  await act(() => vi.advanceTimersByTimeAsync(1));
  const worker = TokenWorker.instances[0]!;
  expect(worker.postMessage).toHaveBeenCalledTimes(1);
  act(() => worker.answer(1));
  expect(screen.getByLabelText('Estimated visible output tokens: 1')).toHaveTextContent(
    '≈ 1 visible output tokens',
  );
  view.rerender(draw('Hello world'));
  view.rerender(draw('Hello world!'));
  await act(() => vi.advanceTimersByTimeAsync(100));
  expect(worker.postMessage).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(100));
  expect(worker.postMessage).toHaveBeenCalledTimes(2);
  expect(worker.postMessage).toHaveBeenLastCalledWith({
    parts: [{ id: 'reply', text: 'Hello world!' }],
    basis: 'unreported',
  });
  act(() => worker.answer(3));
  expect(screen.getByLabelText('Estimated visible output tokens: 3')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Context usage' })).toHaveAccessibleDescription(
    /120 input · 60 output/,
  );
  view.rerender(draw('Hello world!', { inputTokens: 100, outputTokens: 8 }));
  expect(screen.queryByLabelText(/Estimated visible output/)).not.toBeInTheDocument();
  act(() => worker.answer(999));
  expect(screen.queryByLabelText(/Estimated visible output/)).not.toBeInTheDocument();
  expect(screen.getByLabelText('Turn output tokens: 8')).toHaveTextContent('8 output');
  expect(worker.terminate).not.toHaveBeenCalled();
  view.rerender(draw('Hello world! More text.', { inputTokens: 100, outputTokens: 8 }));
  await act(() => vi.advanceTimersByTimeAsync(200));
  expect(worker.postMessage).toHaveBeenLastCalledWith({
    parts: [{ id: 'reply', text: ' More text.' }],
    basis: '100:8',
  });
  act(() => worker.answer(3, '100:8'));
  expect(screen.getByLabelText('Estimated visible output tokens: 3')).toHaveTextContent(
    '+ ≈ 3 newly visible output tokens',
  );
  expect(screen.getByRole('button', { name: 'Context usage' })).toHaveAccessibleDescription(
    /120 input · 60 output/,
  );
});

it('keeps one worker request in flight and releases it on navigation', async () => {
  vi.useFakeTimers();
  vi.stubGlobal('Worker', TokenWorker);
  const view = render(<TurnUsage visibleText={[{ id: 'reply', text: 'one' }]} />);
  await act(() => vi.advanceTimersByTimeAsync(1));
  const worker = TokenWorker.instances[0]!;
  view.rerender(<TurnUsage visibleText={[{ id: 'reply', text: 'one two' }]} />);
  await act(() => vi.advanceTimersByTimeAsync(1000));
  expect(worker.postMessage).toHaveBeenCalledOnce();
  act(() => worker.answer(1));
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(worker.postMessage).toHaveBeenCalledTimes(2);
  view.unmount();
  expect(worker.terminate).toHaveBeenCalledOnce();
  act(() => worker.answer(2));
});

it('loads no tokenizer for completed usage and handles unavailable workers honestly', async () => {
  vi.stubGlobal('Worker', TokenWorker);
  const view = render(<TurnUsage usage={{ inputTokens: 1, outputTokens: 2 }} />);
  expect(TokenWorker.instances).toHaveLength(0);
  view.rerender(<TurnUsage visibleText={[{ id: 'reply', text: 'Hello' }]} />);
  act(() => TokenWorker.instances[0]!.onerror?.());
  expect(screen.getByText(/output estimate unavailable/)).toBeInTheDocument();
  expect(screen.queryByLabelText(/Estimated visible output/)).not.toBeInTheDocument();
});
