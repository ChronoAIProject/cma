import { useEffect, useState } from 'react';
import type { ConversationState } from './model';
import { isPendingTool } from './toolStatus';

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

export function recordedTurnActivity(state: ConversationState) {
  const runId = state.activeRunId;
  const messages = state.messages.filter(
    (item) => item.runId === runId && item.role === 'assistant' && item.content.trim() !== '',
  );
  const tools = state.toolCalls.filter((item) => item.runId === runId);
  const reasoning = state.reasoning.filter(
    (item) => item.runId === runId && item.content.trim() !== '',
  );
  const plans = state.plans.filter((item) => item.runId === runId && item.entries.length > 0);
  const pending = tools.find(isPendingTool);
  const reply = messages.find((message) => message.status === 'streaming');
  const activeReasoning = reasoning.find((item) => item.status === 'streaming');
  const start = Date.parse(state.activeRunStartedAt ?? '');
  const lastAt = [
    ...messages.map(
      (item) => item.progressAt ?? (item.sequence === undefined ? undefined : item.at),
    ),
    ...tools.map((item) => item.progressAt ?? item.finishedAt ?? item.startedAt),
    ...reasoning.map((item) => item.progressAt),
    ...plans.map((item) => item.progressAt),
  ].reduce<string | undefined>((latest, candidate) => {
    const time = Date.parse(candidate ?? '');
    if (!Number.isFinite(time) || time < start) return latest;
    return latest === undefined || time > Date.parse(latest) ? candidate : latest;
  }, undefined);
  if (pending) return { phase: 'Running a tool', since: pending.startedAt, lastAt };
  if (reply) return { phase: 'Responding', since: reply.at, lastAt };
  if (activeReasoning) return { phase: 'Reasoning', since: activeReasoning.startedAt, lastAt };
  return hasVisibleTurnProgress(state)
    ? { phase: 'Waiting for more activity', since: lastAt, lastAt }
    : { phase: 'Waiting for a response', since: state.activeRunStartedAt, lastAt };
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
      Activity needs attention. No new activity observed here for {Math.floor(elapsed / 60_000)}{' '}
      minutes.
    </p>
  );
}
