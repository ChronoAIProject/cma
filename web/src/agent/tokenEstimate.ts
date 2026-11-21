import type { ConversationState } from './model';

export interface VisibleText {
  readonly id: string;
  readonly text: string;
}
export interface VisibleTokenEstimate {
  readonly tokens: number;
  readonly partial: boolean;
}
export const VISIBLE_TEXT_LIMIT = 64 * 1024;
const CHUNK_SIZE = 2048;

/** Current-turn visible model text only. Never inputs, tool output, or earlier turns. */
export function visibleTurnText(conversation: ConversationState): readonly VisibleText[] {
  const run = conversation.activeRunId;
  if (run === undefined) return [];
  // One extra code unit tells the counter that visible text was truncated.
  let remaining = VISIBLE_TEXT_LIMIT + 1;
  const bounded = (items: readonly { readonly content: string }[]) => {
    let text = '';
    for (const item of items) {
      const included = item.content.slice(0, remaining);
      text += included;
      remaining -= included.length;
      if (remaining === 0) break;
    }
    return text;
  };
  return [
    {
      id: 'reply',
      text: bounded(
        conversation.messages.filter((item) => item.runId === run && item.role === 'assistant'),
      ),
    },
    {
      id: 'reasoning',
      text: bounded(conversation.reasoning.filter((item) => item.runId === run)),
    },
  ];
}

/** A restored live fragment may have a new ID. Compare its text boundary, not ID. */
export function newlyVisibleText(parts: readonly VisibleText[], anchor: readonly VisibleText[]) {
  return parts.map((part) => {
    const before = anchor.find((value) => value.id === part.id)?.text ?? '';
    return {
      id: part.id,
      text: part.text.startsWith(before) ? part.text.slice(before.length) : '',
    };
  });
}

/** Stable 2,048-code-unit chunks are counted once; only changed chunks are encoded again.
 * Chunk boundaries and visible-only text make this an estimate of model usage.
 */
export function visibleTokenCounter(countTokens: (text: string) => number) {
  let cached = new Map<string, { text: string; tokens: number }>();
  return (parts: readonly VisibleText[]): VisibleTokenEstimate => {
    let remaining = VISIBLE_TEXT_LIMIT;
    let tokens = 0;
    let partial = false;
    const next = new Map<string, { text: string; tokens: number }>();
    for (const part of parts) {
      const text = part.text.slice(0, remaining);
      partial ||= text.length < part.text.length;
      remaining -= text.length;
      for (let start = 0; start < text.length;) {
        let end = Math.min(start + CHUNK_SIZE, text.length);
        // Do not split a UTF-16 surrogate pair across cache chunks.
        const previous = text.charCodeAt(end - 1);
        if (end < text.length && previous >= 0xd800 && previous <= 0xdbff) end -= 1;
        const key = `${part.id}:${String(start)}`;
        const chunk = text.slice(start, end);
        const old = cached.get(key);
        const counted = old?.text === chunk ? old : { text: chunk, tokens: countTokens(chunk) };
        tokens += counted.tokens;
        next.set(key, counted);
        start = end;
      }
    }
    cached = next;
    return { tokens, partial };
  };
}
