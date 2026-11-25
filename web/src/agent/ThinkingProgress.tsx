import { useEffect, useState } from 'react';
import type { ConversationState } from './model';

export const THINKING_NO_PROGRESS_THRESHOLD_MS = 5 * 60_000;

export function hasVisibleTurnProgress(state: ConversationState): boolean {
  const run = state.activeRunId;
  return (
    state.messages.some(
      (item) => item.runId === run && item.role === 'assistant' && item.content.trim() !== '',
    ) ||
    state.reasoning.some((item) => item.runId === run && item.content.trim() !== '') ||
    state.toolCalls.some((item) => item.runId === run) ||
    state.plans.some((item) => item.runId === run && item.entries.length > 0)
  );
}

/** Only visible work changes restart the local observation window, never reconnects. */
export function thinkingProgressKey(state: ConversationState): string {
  const runId = state.activeRunId;
  return JSON.stringify([
    state.messages
      .filter((item) => item.runId === runId && item.role === 'assistant')
      .map(({ content, status }) => [content, status]),
    state.reasoning.filter((item) => item.runId === runId).map(({ content }) => content),
    state.toolCalls
      .filter((item) => item.runId === runId)
      .map(({ id, title, status, detail, output }) => [id, title, status, detail, output]),
    state.plans.filter((item) => item.runId === runId).map(({ entries }) => entries),
  ]);
}

/** This view's observation age is not a backend deadline or a failed-turn outcome. */
export function ThinkingProgress() {
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const observedAt = Date.now();
    const update = () => setElapsed(Math.max(0, Date.now() - observedAt));
    const timer = window.setInterval(update, 60_000);
    document.addEventListener('visibilitychange', update);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', update);
    };
  }, []);
  if (elapsed < THINKING_NO_PROGRESS_THRESHOLD_MS) return null;
  return (
    <p className="message__delivery" role="status">
      No new activity observed here for {Math.floor(elapsed / 60_000)} minutes.
    </p>
  );
}
